#!/usr/bin/env python3
"""
relay.py — 单文件通用 MCP 中继客户端，改 DEFAULT_URL 即可换远程服务

快速开始（零配置，默认连接 DEFAULT_URL）：
  pip install --quiet --target ./vendor mcp        # 依赖只装一次
  python3 relay.py daemon start                     # 建立并保持一条长连接会话（心跳 / 自动重连）
  python3 relay.py tools --json                     # 查看当前服务端的工具和参数
  python3 relay.py call 工具名 --args '{"参数":"值"}'   # 按 schema 调用工具

子命令：
  daemon  start|status|events|reconnect|stop|log|run   常驻会话（run = 前台运行，start 会在后台拉起 run）
  tools / call / info / resources / read / prompts / prompt / ping   通用 MCP 操作
  probe   一次性诊断     http  通用 HTTP 请求     config show|set|clear  配置

通用选项：--url --token -H "K: V" --transport auto|streamable-http|sse|stdio --mode daemon|auto|direct
          --legacy/--no-legacy --keep-session --timeout --connect-timeout --json
心跳选项：--ping-interval 25 --ping-timeout 10 --ping-failures 3 --reconnect-max-delay 30
守护进程不设空闲退出时限；平台暂停/回收进程时无法保证持续保活，也不会自动唤醒模型回合。
优先级：命令行 > 环境变量 RELAY_URL / RELAY_TOKEN > relay.json > 内置默认值
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import math
import os
import signal
import socket
import subprocess
import sys
import time
import warnings
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable
from urllib.parse import urlsplit

HERE = os.path.dirname(os.path.abspath(__file__))
for _v in (os.path.join(HERE, "vendor"), os.path.join(os.getcwd(), "vendor")):
    if os.path.isdir(_v) and _v not in sys.path:
        sys.path.insert(0, _v)

try:
    import httpx2
    from mcp.client.client import Client
    from mcp.client.sse import sse_client
    from mcp.client.stdio import StdioServerParameters
    from mcp.client.streamable_http import streamable_http_client
    from mcp.shared._httpx_utils import create_mcp_http_client
    from mcp.shared.exceptions import MCPDeprecationWarning, MCPError
except ImportError as e:  # pragma: no cover
    sys.stderr.write(f"[relay] 缺少依赖: {e}\n请运行:  pip install --quiet --target {os.path.join(HERE, 'vendor')} mcp\n")
    sys.exit(2)

warnings.simplefilter("ignore", MCPDeprecationWarning)

# ============================================================================
# 内置默认值（换目标时改这里，或用 relay.py config set --url ...）
# ============================================================================
DEFAULT_URL = "https://xxx.com/mcp"
DEFAULT_TOKEN = ""            # 服务端加了鉴权后填这里（或 config set --token）
DEFAULT_LEGACY = False        # 自动协商；旧服务不兼容时可改 True 或加 --legacy
# 以下只供本地 stdio MCP 使用；远程 HTTP/SSE 不用改。
DEFAULT_COMMAND = ""         # 如 "python3"；为空时使用上面的 DEFAULT_URL
DEFAULT_ARGS = []            # 如 ["/path/to/mcp_server.py"]
DEFAULT_ENV = {}             # stdio 额外环境变量（按服务需要填写）
DEFAULT_CWD = None           # stdio 工作目录（可选）

CONFIG_PATH = os.path.join(HERE, "relay.json")
DAEMON_LOG = os.path.join(HERE, "daemon.log")
DEFAULT_SOCK = os.path.join(HERE, ".relay-daemon.sock")  # 每个目录独立，不影响旧版
TRANSPORTS = ("auto", "streamable-http", "sse", "stdio")
MODES = ("auto", "daemon", "direct")


# ============================================================================
# 配置
# ============================================================================
@dataclass
class Config:
    url: str = DEFAULT_URL
    token: str | None = DEFAULT_TOKEN or None
    headers: dict[str, str] = field(default_factory=dict)
    transport: str = "auto"
    timeout: float = 60.0
    connect_timeout: float = 15.0
    legacy: bool = DEFAULT_LEGACY
    keep_session: bool = False
    mode: str = "daemon"       # 默认复用长连接；没有 daemon 就报错，不偷偷建立一次性会话
    sock: str = DEFAULT_SOCK
    ping_interval: float = 25.0
    ping_timeout: float = 10.0
    ping_failures: int = 3
    reconnect_max_delay: float = 30.0

    def all_headers(self) -> dict[str, str]:
        h = dict(self.headers)
        if self.token:
            tok = self.token.strip()
            h["Authorization"] = tok if " " in tok else f"Bearer {tok}"
        return h

    def to_dict(self) -> dict[str, Any]:
        return {k: getattr(self, k) for k in (
            "url", "token", "headers", "transport", "timeout", "connect_timeout", "legacy",
            "keep_session", "mode", "sock", "ping_interval", "ping_timeout", "ping_failures", "reconnect_max_delay")}


def load_config() -> Config:
    cfg = Config()
    if os.path.exists(CONFIG_PATH):
        try:
            with open(CONFIG_PATH, "r", encoding="utf-8") as f:
                data = json.load(f)
            for k in cfg.to_dict():
                if k in data and data[k] is not None:
                    setattr(cfg, k, data[k])
            cfg.headers = dict(cfg.headers or {})
        except Exception as e:
            sys.stderr.write(f"[relay] 配置文件读取失败，已忽略: {e}\n")
    if os.environ.get("RELAY_URL"):
        cfg.url = os.environ["RELAY_URL"]
    if os.environ.get("RELAY_TOKEN"):
        cfg.token = os.environ["RELAY_TOKEN"]
    return cfg


def save_config(cfg: Config) -> None:
    with open(CONFIG_PATH, "w", encoding="utf-8") as f:
        json.dump(cfg.to_dict(), f, ensure_ascii=False, indent=2)
        f.write("\n")


def parse_header_args(items: list[str] | None) -> dict[str, str]:
    out: dict[str, str] = {}
    for item in items or []:
        if ":" not in item:
            raise SystemExit(f"-H 格式应为 'Key: Value'，收到: {item!r}")
        k, v = item.split(":", 1)
        out[k.strip()] = v.strip()
    return out


def apply_overrides(cfg: Config, args: argparse.Namespace) -> Config:
    g = lambda n: getattr(args, n, None)  # noqa: E731
    if g("url"):
        cfg.url = args.url
    if g("token") is not None:
        cfg.token = args.token or None
    if g("header"):
        cfg.headers.update(parse_header_args(args.header))
    if g("transport"):
        cfg.transport = args.transport
    if g("timeout") is not None:
        cfg.timeout = float(args.timeout)
    if g("connect_timeout") is not None:
        cfg.connect_timeout = float(args.connect_timeout)
    if g("legacy") is not None:
        cfg.legacy = bool(args.legacy)
    if g("keep_session"):
        cfg.keep_session = True
    if g("mode"):
        cfg.mode = args.mode
    if g("sock"):
        cfg.sock = args.sock
    if g("ping_interval") is not None:
        cfg.ping_interval = float(args.ping_interval)
    if g("ping_timeout") is not None:
        cfg.ping_timeout = float(args.ping_timeout)
    if g("ping_failures") is not None:
        cfg.ping_failures = int(args.ping_failures)
    if g("reconnect_max_delay") is not None:
        cfg.reconnect_max_delay = float(args.reconnect_max_delay)
    for name in ("timeout", "connect_timeout", "ping_interval", "ping_timeout", "reconnect_max_delay"):
        value = float(getattr(cfg, name))
        if not math.isfinite(value) or value <= 0:
            raise SystemExit(f"{name} 必须是有限正数")
        setattr(cfg, name, value)
    if isinstance(cfg.ping_failures, bool) or not isinstance(cfg.ping_failures, int) or cfg.ping_failures < 1:
        raise SystemExit("ping_failures 必须是正整数")
    return cfg


def mask(s: str | None) -> str:
    if not s:
        return "(未设置)"
    return "*" * len(s) if len(s) <= 8 else s[:4] + "…" + s[-4:]


def host_of(u: str | None) -> str:
    try:
        return (urlsplit(u or "").hostname or "").lower()
    except Exception:
        return ""


def now_iso() -> str:
    return dt.datetime.now().astimezone().isoformat(timespec="seconds")


# ============================================================================
# 输出 / 错误
# ============================================================================
def dump(obj: Any) -> Any:
    if hasattr(obj, "model_dump"):
        return obj.model_dump(mode="json", by_alias=True, exclude_none=True)
    if isinstance(obj, list):
        return [dump(x) for x in obj]
    if isinstance(obj, dict):
        return {k: dump(v) for k, v in obj.items()}
    return obj


def pj(obj: Any) -> None:
    print(json.dumps(dump(obj), ensure_ascii=False, indent=2))


class ConnectFailed(Exception):
    def __init__(self, attempts: list[tuple[str, str, str]]):
        self.attempts = attempts
        lines = ["无法与目标建立 MCP 会话，尝试记录："]
        for kind, url, err in attempts:
            lines.append(f"  - [{kind}] {url}\n      -> {err}")
        super().__init__("\n".join(lines))


class _FnError(Exception):
    def __init__(self, inner: BaseException):
        self.inner = inner


class _Dropped(Exception):
    def __init__(self, inner: BaseException):
        self.inner = inner


def _flatten(e: BaseException):
    if isinstance(e, BaseExceptionGroup):
        for x in e.exceptions:
            yield from _flatten(x)
    else:
        yield e


def short_err(e: BaseException) -> str:
    if isinstance(e, BaseExceptionGroup):
        inner = [x for x in (short_err(x) for x in e.exceptions) if x]
        return "; ".join(dict.fromkeys(inner)) or type(e).__name__
    if isinstance(e, httpx2.HTTPStatusError):
        r = e.response
        try:
            body = (r.text or "")[:160].replace("\n", " ")
        except Exception:
            body = ""
        hint = {401: "（鉴权失败：检查 --token / -H）", 403: "（鉴权失败：检查 --token / -H）",
                421: "（服务端 Host 校验拒绝：关闭 DNS rebinding 保护或加入 allowed_hosts）"}.get(r.status_code, "")
        return f"HTTP {r.status_code} {r.reason_phrase} {body} {hint}".strip()
    if isinstance(e, httpx2.ConnectError):
        return f"连接失败: {e}"
    if isinstance(e, httpx2.TimeoutException):
        return f"超时: {type(e).__name__}"
    if isinstance(e, MCPError):
        return f"MCP 错误 {e.error.code}: {e.error.message}" if hasattr(e, "error") else f"MCP 错误: {e}"
    msg = str(e).strip()
    return f"{type(e).__name__}: {msg}" if msg else type(e).__name__


# ============================================================================
# 连接
# ============================================================================
def candidates(cfg: Config) -> list[tuple[str, str]]:
    if DEFAULT_COMMAND or cfg.transport == "stdio":
        if not DEFAULT_COMMAND:
            raise ValueError("stdio 需要在文件顶部填写 DEFAULT_COMMAND / DEFAULT_ARGS")
        return [("stdio", DEFAULT_COMMAND)]
    url = cfg.url.strip()
    base = url.rstrip("/")
    out: list[tuple[str, str]] = []

    def add(kind: str, u: str) -> None:
        if (kind, u) not in out:
            out.append((kind, u))

    if cfg.transport == "streamable-http":
        add("streamable-http", url)
        if not base.endswith("/mcp"):
            add("streamable-http", base + "/mcp")
    elif cfg.transport == "sse":
        add("sse", url)
        if not base.endswith("/sse"):
            add("sse", base + "/sse")
    elif base.endswith("/sse"):
        add("sse", url)
        add("streamable-http", base[: -len("/sse")] + "/mcp")
    else:
        add("streamable-http", url)
        if not base.endswith("/mcp"):
            add("streamable-http", base + "/mcp")
        add("sse", base + "/sse")
        add("sse", url)
    return out


class SessionTracker:
    def __init__(self) -> None:
        self.session_id: str | None = None

    async def on_response(self, resp: httpx2.Response) -> None:
        sid = resp.headers.get("mcp-session-id")
        if sid:
            self.session_id = sid


def open_transport(kind: str, url: str, cfg: Config, tracker: SessionTracker | None = None,
                   read_timeout: float | None = None):
    if kind == "stdio":
        return None, StdioServerParameters(command=DEFAULT_COMMAND, args=DEFAULT_ARGS,
                                            env=DEFAULT_ENV or None, cwd=DEFAULT_CWD)
    headers = cfg.all_headers()
    rt = read_timeout if read_timeout is not None else max(cfg.timeout, 30.0)
    if kind == "streamable-http":
        hc = create_mcp_http_client(headers=headers, timeout=httpx2.Timeout(cfg.connect_timeout, read=rt))
        if tracker is not None:
            hc.event_hooks = {"response": [tracker.on_response]}
        return hc, streamable_http_client(url, http_client=hc, terminate_on_close=not cfg.keep_session)
    return None, sse_client(url, headers=headers, timeout=cfg.connect_timeout, sse_read_timeout=rt)


def client_kwargs(cfg: Config, **extra: Any) -> dict[str, Any]:
    kw: dict[str, Any] = {"read_timeout_seconds": cfg.timeout}
    if cfg.legacy:
        kw["mode"] = "legacy"
    kw.update(extra)
    return kw


async def _run_one(kind: str, url: str, cfg: Config, fn: Callable[[Client, str, str], Awaitable[Any]]) -> Any:
    tracker = SessionTracker()
    hc, transport = open_transport(kind, url, cfg, tracker)

    async def body() -> Any:
        async with Client(transport, **client_kwargs(cfg)) as client:
            client._relay_session = tracker  # type: ignore[attr-defined]
            try:
                return await fn(client, kind, url)
            except Exception as e:
                raise _FnError(e) from e

    if hc is not None:
        async with hc:
            return await body()
    return await body()


async def _http_status_hint(url: str, cfg: Config) -> str:
    body = {"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "relay-probe", "version": "0"}}}
    headers = {**cfg.all_headers(), "Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    try:
        async with httpx2.AsyncClient(timeout=cfg.connect_timeout, headers=headers) as hc:
            r = await hc.post(url, json=body)
            try:
                text = (r.text or "")[:120].replace("\n", " ")
            except Exception:
                text = ""
            extra = {401: "（鉴权失败）", 403: "（鉴权失败）", 421: "（Host 校验拒绝）",
                     404: "（路径不对或不是 Streamable HTTP 端点）", 405: "（路径不对或不是 Streamable HTTP 端点）"}.get(r.status_code, "")
            return f"直接 POST 返回 HTTP {r.status_code} {r.reason_phrase} {text} {extra}".strip()
    except Exception as e:  # noqa: BLE001
        return f"直接 POST 也失败: {short_err(e)}"


async def _attempt_error(kind: str, url: str, cfg: Config, e: BaseException) -> str:
    msg = short_err(e)
    if kind == "streamable-http" and "-32603" in msg:
        msg += " | " + await _http_status_hint(url, cfg)
    return msg


async def with_client(cfg: Config, fn: Callable[[Client, str, str], Awaitable[Any]]) -> Any:
    attempts: list[tuple[str, str, str]] = []
    for kind, url in candidates(cfg):
        try:
            return await _run_one(kind, url, cfg, fn)
        except _FnError as fe:
            raise fe.inner
        except BaseExceptionGroup as eg:
            fe = next((x for x in _flatten(eg) if isinstance(x, _FnError)), None)
            if fe is not None:
                raise fe.inner
            attempts.append((kind, url, await _attempt_error(kind, url, cfg, eg)))
        except Exception as e:
            attempts.append((kind, url, await _attempt_error(kind, url, cfg, e)))
    raise ConnectFailed(attempts)


# ============================================================================
# 统一操作：perform(client, op, payload) -> JSON dict（direct 模式与守护进程共用）
# ============================================================================
def server_summary(client: Client, kind: str, url: str) -> dict[str, Any]:
    info, caps = client.server_info, client.server_capabilities
    tracker = getattr(client, "_relay_session", None)
    return {"transport": kind, "url": url, "protocolVersion": client.protocol_version,
            "sessionId": getattr(tracker, "session_id", None),
            "server": dump(info) if info else None, "capabilities": dump(caps) if caps else None,
            "instructions": client.instructions}


async def perform(client: Client, op: str, payload: dict[str, Any]) -> Any:
    args = payload.get("arguments") or None
    if op == "tools":
        return dump(await client.list_tools())
    if op == "call":
        rt = payload.get("timeout")
        return dump(await client.call_tool(payload["name"], args, read_timeout_seconds=float(rt) if rt else None))
    if op == "resources":
        out: dict[str, Any] = {}
        try:
            out.update(dump(await client.list_resources()))
        except Exception as e:
            out["resourcesError"] = short_err(e)
        try:
            out.update(dump(await client.list_resource_templates()))
        except Exception as e:
            out["templatesError"] = short_err(e)
        return out
    if op == "read":
        return dump(await client.read_resource(payload["uri"]))
    if op == "prompts":
        return dump(await client.list_prompts())
    if op == "prompt":
        a = {k: str(v) for k, v in (args or {}).items()} or None
        return dump(await client.get_prompt(payload["name"], a))
    if op == "ping":
        t0 = time.time()
        try:
            await client.send_ping()
            return {"ms": round((time.time() - t0) * 1000), "via": "ping"}
        except MCPError as e:
            if getattr(getattr(e, "error", None), "code", None) != -32601:
                raise
        t0 = time.time()
        caps = dump(client.server_capabilities) or {}
        for cap, method in (("tools", "list_tools"), ("resources", "list_resources"), ("prompts", "list_prompts")):
            if cap in caps:
                await getattr(client, method)(cache_mode="bypass")
                return {"ms": round((time.time() - t0) * 1000), "via": method}
        return {"ms": None, "via": "unsupported"}  # 没有可用的只读探活能力
    raise ValueError(f"未知操作: {op}")


# ============================================================================
# 守护进程通道（unix socket，一行 JSON 请求 / 一行 JSON 应答）
# ============================================================================
def daemon_request(cfg: Config, op: str, payload: dict[str, Any] | None = None, timeout: float | None = None) -> dict[str, Any]:
    req: dict[str, Any] = {"op": op, "payload": payload or {}}
    if timeout is not None:
        req["timeout"] = timeout
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout((timeout or cfg.timeout) + 15)
    try:
        s.connect(cfg.sock)
        s.sendall((json.dumps(req, ensure_ascii=False) + "\n").encode())
        s.shutdown(socket.SHUT_WR)
        chunks: list[bytes] = []
        while True:
            b = s.recv(1 << 16)
            if not b:
                break
            chunks.append(b)
    finally:
        s.close()
    raw = b"".join(chunks).decode("utf-8", "replace").strip()
    if not raw:
        raise RuntimeError("守护进程没有返回数据")
    return json.loads(raw)


def daemon_alive(cfg: Config) -> dict[str, Any] | None:
    if not os.path.exists(cfg.sock):
        return None
    try:
        r = daemon_request(cfg, "status", timeout=5)
        return r.get("result") if r.get("ok") else None
    except Exception:
        return None


def choose_backend(cfg: Config, args: argparse.Namespace) -> tuple[str, dict[str, Any] | None]:
    if cfg.mode == "direct":
        return "direct", None
    st = daemon_alive(cfg)
    if st is None:
        if cfg.mode == "daemon":
            raise SystemExit("守护进程未运行：先执行  python3 relay.py daemon start")
        return "direct", None
    if (st.get("configuredUrl") != cfg.url or st.get("command", "") != DEFAULT_COMMAND
            or st.get("commandArgs", []) != DEFAULT_ARGS):
        raise SystemExit("目标配置已改变。请先 daemon stop 再 daemon start，避免调用旧服务。")
    explicit = getattr(args, "url", None)
    if explicit and explicit.rstrip("/") not in ((st.get("url") or "").rstrip("/"), (st.get("configuredUrl") or "").rstrip("/")):
        if cfg.mode == "daemon":
            raise SystemExit(f"守护进程连接的是 {st.get('url')}，与 --url 不同")
        return "direct", None
    return "daemon", st


async def execute(cfg: Config, args: argparse.Namespace, op: str, payload: dict[str, Any] | None = None) -> tuple[Any, str]:
    backend, _ = choose_backend(cfg, args)
    payload = payload or {}
    if backend == "daemon":
        r = daemon_request(cfg, op, payload, timeout=float(payload.get("timeout") or cfg.timeout))
        if not r.get("ok"):
            raise RuntimeError(f"[daemon] {r.get('error')}")
        return r.get("result"), "daemon"

    async def fn(client: Client, k: str, u: str):
        return server_summary(client, k, u) if op == "info" else await perform(client, op, payload)
    return await with_client(cfg, fn), "direct"


# ============================================================================
# 守护进程本体
# ============================================================================
class Daemon:
    def __init__(self, cfg: Config) -> None:
        self.cfg = cfg
        self.client: Client | None = None
        self.connected = False
        self.transport: str | None = None
        self.url: str | None = None
        self.tracker: SessionTracker | None = None
        self.summary: dict[str, Any] = {}
        self.started = time.time()
        self.connected_at: float | None = None
        self.connected_monotonic: float | None = None
        self.last_ping_at: str | None = None
        self.last_ping_attempt_at: str | None = None
        self.last_ping_error: str | None = None
        self.consecutive_ping_failures = 0
        self.last_ping_ms: int | None = None
        self.last_ping_via: str | None = None
        self.reconnects = 0
        self.last_error: str | None = None
        self.events: deque[dict[str, Any]] = deque(maxlen=500)
        self.stopping = False
        self._drop = asyncio.Event()

    def log(self, msg: str) -> None:
        print(f"[{now_iso()}] {msg}", flush=True)

    async def on_message(self, message: Any) -> None:
        if isinstance(message, Exception):
            kind, summary = "error", short_err(message)
        else:
            kind = type(message).__name__
            try:
                summary = json.dumps(dump(message), ensure_ascii=False)[:400]
            except Exception:
                summary = str(message)[:400]
        self.events.append({"at": now_iso(), "kind": kind, "summary": summary})
        self.log(f"event {kind}: {summary[:200]}")

    async def on_log(self, params: Any) -> None:
        try:
            data = json.dumps(dump(params), ensure_ascii=False)
        except Exception:
            data = str(params)
        self.events.append({"at": now_iso(), "kind": "server-log", "summary": data[:400]})
        self.log(f"server-log: {data[:200]}")

    async def hold(self, kind: str, url: str) -> None:
        tracker = SessionTracker()
        hc, transport = open_transport(kind, url, self.cfg, tracker, read_timeout=3600.0)
        established = False

        async def body() -> None:
            nonlocal established
            kw = client_kwargs(self.cfg, message_handler=self.on_message, logging_callback=self.on_log)
            async with Client(transport, **kw) as client:
                established = True
                client._relay_session = tracker  # type: ignore[attr-defined]
                self.client, self.connected = client, True
                self.transport, self.url, self.tracker = kind, url, tracker
                self.connected_at = time.time()
                self.connected_monotonic = time.monotonic()
                self.last_error = None
                self.last_ping_at = self.last_ping_attempt_at = None
                self.last_ping_ms = self.last_ping_via = None
                self.last_ping_error = None
                self.consecutive_ping_failures = 0
                self.summary = server_summary(client, kind, url)
                srv = self.summary.get("server") or {}
                self.log(f"已连接 [{kind}] {url}  会话={tracker.session_id or '(无状态)'}  "
                         f"服务端={srv.get('name')} {srv.get('version', '')}  协议={self.summary.get('protocolVersion')}")
                try:
                    await self.ping_loop(client)
                finally:
                    self.client, self.connected = None, False
                    self.log("连接已关闭")

        try:
            if hc is not None:
                async with hc:
                    await body()
            else:
                await body()
        except asyncio.CancelledError:
            raise  # 平台取消/进程退出时不要吞掉取消并自动重连
        except Exception as e:
            if established:
                raise _Dropped(e) from e
            raise

    async def ping_loop(self, client: Client) -> None:
        interval = max(5.0, self.cfg.ping_interval)
        while not self._drop.is_set():
            try:
                await asyncio.wait_for(self._drop.wait(), timeout=interval)
                return
            except asyncio.TimeoutError:
                pass
            if self.stopping:
                return
            self.last_ping_attempt_at = now_iso()
            try:
                r = await asyncio.wait_for(perform(client, "ping", {}), timeout=self.cfg.ping_timeout)
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self.consecutive_ping_failures += 1
                self.last_ping_error = short_err(e)
                self.log(f"心跳失败 {self.consecutive_ping_failures}/{self.cfg.ping_failures}: {self.last_ping_error}")
                if self.consecutive_ping_failures >= self.cfg.ping_failures:
                    raise RuntimeError(f"连续 {self.consecutive_ping_failures} 次心跳失败: {self.last_ping_error}") from e
                continue  # 瞬时抖动不拆掉现有会话，下个周期再试
            if self.consecutive_ping_failures:
                self.log("心跳恢复，继续复用原会话")
            self.consecutive_ping_failures = 0
            self.last_ping_error = None
            self.last_ping_ms, self.last_ping_via = r.get("ms"), r.get("via")
            # unsupported 不是成功探活；记录尝试时间但不冒充成功心跳。
            if r.get("via") != "unsupported":
                self.last_ping_at = now_iso()

    async def connection_loop(self) -> None:
        backoff = 1.0
        while not self.stopping:
            self._drop = asyncio.Event()
            attempts: list[tuple[str, str, str]] = []
            dropped: _Dropped | None = None
            had_connection = False
            for kind, url in candidates(self.cfg):
                try:
                    await self.hold(kind, url)
                    had_connection = True
                    break
                except _Dropped as d:
                    dropped, had_connection = d, True
                    break
                except asyncio.CancelledError:
                    raise
                except Exception as e:  # noqa: BLE001
                    attempts.append((kind, url, await _attempt_error(kind, url, self.cfg, e)))
                if self.stopping:
                    break
            if dropped is not None:
                self.last_error = f"连接中断: {short_err(dropped.inner)}"
                self.log(self.last_error)
            elif not had_connection:
                self.last_error = str(ConnectFailed(attempts))
                self.log(self.last_error)
            if self.stopping:
                break
            self.reconnects += 1
            if had_connection and self._drop.is_set() and dropped is None:
                backoff = 1.0
                await asyncio.sleep(0.2)
            else:
                # 短暂握手成功后立即掉线也应退避，避免持续快速重连。
                if (had_connection and self.connected_monotonic is not None
                        and time.monotonic() - self.connected_monotonic >= 60.0):
                    backoff = 1.0
                delay = min(backoff, self.cfg.reconnect_max_delay)
                self.log(f"{delay:g}s 后重连…")
                try:
                    await asyncio.wait_for(self._drop.wait(), timeout=delay)
                    backoff = 1.0  # stop / 显式 reconnect 可打断退避
                except asyncio.TimeoutError:
                    backoff = min(delay * 2, self.cfg.reconnect_max_delay)

    def status(self) -> dict[str, Any]:
        return {"pid": os.getpid(), "uptimeSec": round(time.time() - self.started), "connected": self.connected,
                "configuredUrl": self.cfg.url, "command": DEFAULT_COMMAND, "commandArgs": DEFAULT_ARGS,
                "url": self.url, "transport": self.transport,
                "sessionId": self.tracker.session_id if self.tracker else None,
                "server": self.summary.get("server"), "protocolVersion": self.summary.get("protocolVersion"),
                "connectedSec": round(time.time() - self.connected_at) if (self.connected and self.connected_at) else 0,
                "reconnects": self.reconnects, "lastPingMs": self.last_ping_ms, "lastPingVia": self.last_ping_via,
                "lastPingAt": self.last_ping_at, "lastError": self.last_error, "eventCount": len(self.events),
                "legacy": self.cfg.legacy, "pingInterval": self.cfg.ping_interval,
                "pingTimeout": self.cfg.ping_timeout, "pingFailureThreshold": self.cfg.ping_failures,
                "consecutivePingFailures": self.consecutive_ping_failures,
                "lastPingAttemptAt": self.last_ping_attempt_at, "lastPingError": self.last_ping_error,
                "reconnectMaxDelay": self.cfg.reconnect_max_delay}

    async def dispatch(self, req: dict[str, Any]) -> dict[str, Any]:
        op = req.get("op")
        payload = req.get("payload") or {}
        timeout = float(req.get("timeout") or self.cfg.timeout)
        if op == "status":
            return {"ok": True, "result": self.status()}
        if op == "events":
            return {"ok": True, "result": list(self.events)[-int(payload.get("n") or 50):]}
        if op == "reconnect":
            self.log("收到 reconnect 请求")
            self._drop.set()
            return {"ok": True, "result": "reconnecting"}
        if op == "shutdown":
            self.log("收到 shutdown 请求")
            self.stopping = True
            self._drop.set()
            return {"ok": True, "result": "bye"}
        if not self.connected or self.client is None:
            return {"ok": False, "error": f"当前未连接到服务端：{self.last_error or '正在建立连接'}"}
        if op == "info":
            s = dict(self.summary)
            s["sessionId"] = self.tracker.session_id if self.tracker else None
            return {"ok": True, "result": s}
        try:
            result = await asyncio.wait_for(perform(self.client, op, payload), timeout=timeout + 5)
            return {"ok": True, "result": result}
        except asyncio.TimeoutError:
            return {"ok": False, "error": f"操作超时（{timeout:g}s）"}
        except Exception as e:  # noqa: BLE001
            return {"ok": False, "error": short_err(e)}

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            data = await asyncio.wait_for(reader.read(16 * 1024 * 1024), timeout=30)
            req = json.loads(data.decode("utf-8") or "{}")
            t0 = time.time()
            resp = await self.dispatch(req)
            if req.get("op") not in ("status", "events"):
                self.log(f"{req.get('op')} {json.dumps(req.get('payload') or {}, ensure_ascii=False)[:120]} "
                         f"-> {'ok' if resp.get('ok') else 'ERR ' + str(resp.get('error'))[:120]} ({round((time.time() - t0) * 1000)} ms)")
        except Exception as e:  # noqa: BLE001
            resp = {"ok": False, "error": short_err(e)}
        try:
            writer.write((json.dumps(resp, ensure_ascii=False) + "\n").encode("utf-8"))
            await writer.drain()
        except Exception:
            pass
        finally:
            try:
                writer.close()
            except Exception:
                pass


async def daemon_main(cfg: Config) -> int:
    d = Daemon(cfg)
    sock, pidfile = cfg.sock, cfg.sock + ".pid"
    if os.path.exists(sock):
        try:
            r = daemon_request(cfg, "status", timeout=3)
            if r.get("ok"):
                print(f"已有守护进程在运行 (pid={r['result'].get('pid')})，退出。", flush=True)
                return 1
        except Exception:
            pass
        os.unlink(sock)
    server = await asyncio.start_unix_server(d.handle, path=sock, limit=16 * 1024 * 1024)
    os.chmod(sock, 0o600)
    with open(pidfile, "w") as f:
        f.write(str(os.getpid()))
    d.log(f"守护进程启动 pid={os.getpid()}  socket={sock}  目标={cfg.url}  传输={cfg.transport}  legacy={cfg.legacy}  心跳={cfg.ping_interval}s")
    loop = asyncio.get_running_loop()

    def _sig(*_: Any) -> None:
        d.log("收到终止信号")
        d.stopping = True
        d._drop.set()

    for s in (signal.SIGTERM, signal.SIGINT):
        try:
            loop.add_signal_handler(s, _sig)
        except NotImplementedError:  # pragma: no cover
            pass
    try:
        await d.connection_loop()
    finally:
        server.close()
        await server.wait_closed()
        for p in (sock, pidfile):
            try:
                os.unlink(p)
            except FileNotFoundError:
                pass
        d.log("守护进程退出")
    return 0


# ============================================================================
# 打印
# ============================================================================
def print_info(res: dict[str, Any]) -> None:
    srv = res.get("server") or {}
    print(f"传输: {res.get('transport')}    地址: {res.get('url')}")
    print(f"服务端: {srv.get('name')} {srv.get('version', '')}    协议版本: {res.get('protocolVersion')}    会话: {res.get('sessionId') or '(无状态/未知)'}")
    print("能力: " + (", ".join((res.get("capabilities") or {}).keys()) or "-"))
    if res.get("instructions"):
        print("说明: " + str(res["instructions"]))


def print_tools(res: dict[str, Any]) -> None:
    tools = res.get("tools") or []
    if not tools:
        print("(服务端没有暴露任何工具)")
        return
    for t in tools:
        schema = t.get("inputSchema") or {}
        props = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", []) if isinstance(schema, dict) else [])
        params = [f"{n}{'*' if n in required else ''}: {(s.get('type', 'any') if isinstance(s, dict) else 'any')}" for n, s in props.items()]
        print(f"• {t['name']}({', '.join(params)})")
        if t.get("description"):
            print(f"    {t['description'].strip().splitlines()[0]}")
    print(f"\n共 {len(tools)} 个工具（带 * 为必填参数）。用 --json 查看完整 schema。")


def print_call(res: dict[str, Any]) -> int:
    is_err = bool(res.get("isError"))
    for c in res.get("content") or []:
        ctype = c.get("type")
        if ctype == "text":
            print(c.get("text", ""))
        elif ctype in ("image", "audio"):
            print(f"[{ctype} {c.get('mimeType')}, {len(c.get('data', ''))} base64 chars]")
        else:
            print(json.dumps(c, ensure_ascii=False, indent=2))
    if res.get("structuredContent"):
        print("--- structuredContent ---")
        print(json.dumps(res["structuredContent"], ensure_ascii=False, indent=2))
    if is_err:
        print("(工具返回 isError=true)", file=sys.stderr)
    return 1 if is_err else 0


def print_resources(res: dict[str, Any]) -> None:
    if "resourcesError" in res:
        print(f"resources/list 失败: {res['resourcesError']}")
    else:
        rs = res.get("resources") or []
        for r in rs:
            print(f"• {r.get('uri')}   {r.get('name') or ''}   {r.get('mimeType') or ''}")
        if not rs:
            print("(没有静态资源)")
    ts = res.get("resourceTemplates") or []
    if ts:
        print("--- 模板 ---")
        for t in ts:
            print(f"• {t.get('uriTemplate')}   {t.get('name') or ''}")


def print_read(res: dict[str, Any]) -> None:
    for c in res.get("contents") or []:
        print(c["text"] if "text" in c else f"[blob {c.get('mimeType', '')}, {len(c.get('blob', ''))} base64 chars]")


def print_prompts(res: dict[str, Any]) -> None:
    ps = res.get("prompts") or []
    for p in ps:
        a = ", ".join(f"{x.get('name')}{'*' if x.get('required') else ''}" for x in (p.get("arguments") or []))
        print(f"• {p['name']}({a})")
        if p.get("description"):
            print(f"    {p['description'].strip().splitlines()[0]}")
    if not ps:
        print("(没有 prompt)")


def print_prompt(res: dict[str, Any]) -> None:
    for m in res.get("messages") or []:
        c = m.get("content") or {}
        print(f"[{m.get('role')}] {c.get('text') if c.get('text') is not None else json.dumps(c, ensure_ascii=False)}")


def print_status(st: dict[str, Any]) -> None:
    srv = st.get("server") or {}
    print(f"守护进程 pid={st.get('pid')}  运行 {st.get('uptimeSec')}s  状态: {'已连接 ✅' if st.get('connected') else '未连接 ⏳（重连中）'}")
    print(f"目标: {st.get('url') or st.get('configuredUrl')}  传输: {st.get('transport') or '-'}  会话: {st.get('sessionId') or '-'}")
    if srv:
        print(f"服务端: {srv.get('name')} {srv.get('version', '')}  协议: {st.get('protocolVersion')}")
    print(f"已连接时长: {st.get('connectedSec')}s  重连次数: {st.get('reconnects')}  最近心跳: {st.get('lastPingMs')}ms ({st.get('lastPingVia') or '-'}) @ {st.get('lastPingAt') or '-'}")
    print(f"心跳连续失败: {st.get('consecutivePingFailures', 0)}/{st.get('pingFailureThreshold', 3)}  "
          f"最近尝试: {st.get('lastPingAttemptAt') or '-'}")
    if st.get("lastPingError"):
        print(f"最近心跳错误: {st['lastPingError']}")
    if st.get("lastError"):
        print(f"最近错误: {st['lastError']}")
    print(f"服务端推送事件: {st.get('eventCount')} 条（relay.py daemon events 查看）")


# ============================================================================
# 通用 MCP 子命令
# ============================================================================
def parse_json_arg(s: str | None, what: str) -> dict[str, Any]:
    if not s:
        return {}
    try:
        v = json.loads(s)
    except json.JSONDecodeError as e:
        raise SystemExit(f"{what} 不是合法 JSON: {e}")
    if not isinstance(v, dict):
        raise SystemExit(f"{what} 必须是 JSON 对象")
    return v


async def cmd_probe(cfg: Config, args: argparse.Namespace) -> int:
    headers = cfg.all_headers()
    report: dict[str, Any] = {"url": cfg.url, "http": None, "mcp": [], "daemon": daemon_alive(cfg)}
    try:
        if DEFAULT_COMMAND:
            raise RuntimeError("stdio 不做 HTTP 检查，直接进行 MCP 握手")
        async with httpx2.AsyncClient(timeout=httpx2.Timeout(5.0), headers=headers, follow_redirects=True) as hc:
            t0 = time.time()
            async with hc.stream("GET", cfg.url) as r:
                report["http"] = {"status": r.status_code, "contentType": r.headers.get("content-type"),
                                  "server": r.headers.get("server"), "ms": round((time.time() - t0) * 1000)}
    except httpx2.TimeoutException:
        report["http"] = {"error": "5 秒内没有响应头（服务端可能把无会话的 GET 当作 SSE 长连接在等，不影响 MCP 使用）"}
    except Exception as e:
        report["http"] = {"error": short_err(e)}

    for kind, url in candidates(cfg):
        entry: dict[str, Any] = {"transport": kind, "url": url}
        t0 = time.time()
        try:
            async def fn(client: Client, k: str, u: str):
                tools = await client.list_tools()
                s = server_summary(client, k, u)
                s["toolCount"], s["tools"] = len(tools.tools), [t.name for t in tools.tools]
                return s
            res = await _run_one(kind, url, cfg, fn)
            entry.update({"ok": True, "ms": round((time.time() - t0) * 1000), **res})
        except _FnError as fe:
            entry.update({"ok": False, "ms": round((time.time() - t0) * 1000), "error": "握手成功但 tools/list 失败: " + short_err(fe.inner)})
        except BaseException as e:  # noqa: BLE001
            entry.update({"ok": False, "ms": round((time.time() - t0) * 1000), "error": await _attempt_error(kind, url, cfg, e)})
        report["mcp"].append(entry)
        if entry.get("ok"):
            break

    if args.json:
        pj(report)
        return 0 if any(m.get("ok") for m in report["mcp"]) else 1
    print("\n== 原始 HTTP ==")
    h = report["http"]
    print(f"GET {cfg.url} -> " + (h["error"] if "error" in h else f"{h['status']} ({h['ms']} ms)  content-type={h['contentType']}  server={h['server']}"))
    print("\n== MCP 握手 ==")
    ok_any = False
    for m in report["mcp"]:
        if m.get("ok"):
            ok_any = True
            srv = m.get("server") or {}
            print(f"✅ [{m['transport']}] {m['url']}  ({m['ms']} ms)")
            print(f"   服务端: {srv.get('name')} {srv.get('version', '')}   协议: {m.get('protocolVersion')}   会话: {m.get('sessionId') or '(无状态)'}")
            print(f"   工具 {m['toolCount']} 个: {', '.join(m['tools']) or '-'}")
        else:
            print(f"❌ [{m['transport']}] {m['url']}  ({m['ms']} ms)\n   {m['error']}")
    print("\n== 守护进程 ==")
    d = report["daemon"]
    print(f"运行中: {'已连接' if d.get('connected') else '未连接'}  {d.get('url')}  会话 {d.get('sessionId') or '-'}" if d
          else "未运行。要长期保持同一个会话，请执行: python3 relay.py daemon start")
    if ok_any:
        print(f"\n结论：可以连接。注意 probe 本身是一次性会话（结束时{'不发送' if cfg.keep_session else '会发送'} DELETE）。")
        return 0
    print("\n结论：目前无法建立 MCP 会话。常见原因：地址/路径不对、未带 token、服务端 Host 校验、隧道未启动。")
    return 1


async def cmd_info(cfg: Config, args: argparse.Namespace) -> int:
    res, backend = await execute(cfg, args, "info")
    if args.json:
        pj(res)
    else:
        print_info(res)
        print(f"（经由 {backend}）")
    return 0


async def cmd_tools(cfg: Config, args: argparse.Namespace) -> int:
    res, _ = await execute(cfg, args, "tools")
    pj(res) if args.json else print_tools(res)
    return 0


async def cmd_call(cfg: Config, args: argparse.Namespace) -> int:
    if getattr(args, "args_file", None):
        with open(args.args_file, "r", encoding="utf-8") as f:
            arguments = parse_json_arg(f.read(), "--args-file")
    else:
        arguments = parse_json_arg(args.args, "--args")
    res, _ = await execute(cfg, args, "call", {"name": args.tool, "arguments": arguments, "timeout": cfg.timeout})
    if args.json:
        pj(res)
        return 1 if res.get("isError") else 0
    return print_call(res)


async def cmd_resources(cfg: Config, args: argparse.Namespace) -> int:
    res, _ = await execute(cfg, args, "resources")
    pj(res) if args.json else print_resources(res)
    return 0


async def cmd_read(cfg: Config, args: argparse.Namespace) -> int:
    res, _ = await execute(cfg, args, "read", {"uri": args.uri})
    pj(res) if args.json else print_read(res)
    return 0


async def cmd_prompts(cfg: Config, args: argparse.Namespace) -> int:
    res, _ = await execute(cfg, args, "prompts")
    pj(res) if args.json else print_prompts(res)
    return 0


async def cmd_prompt(cfg: Config, args: argparse.Namespace) -> int:
    res, _ = await execute(cfg, args, "prompt", {"name": args.name, "arguments": parse_json_arg(args.args, "--args")})
    pj(res) if args.json else print_prompt(res)
    return 0


async def cmd_ping(cfg: Config, args: argparse.Namespace) -> int:
    res, backend = await execute(cfg, args, "ping")
    print(json.dumps(res) if args.json else f"pong {res.get('ms')} ms（{res.get('via')}，经由 {backend}）")
    return 0


async def cmd_http(cfg: Config, args: argparse.Namespace) -> int:
    # 保存的凭据只发给与配置地址同一主机的目标；其他主机只用本次命令行显式给的 --token / -H
    explicit = Config(url=cfg.url, token=args.token, headers=parse_header_args(args.header))
    if host_of(args.target) and host_of(args.target) == host_of(cfg.url):
        headers = cfg.all_headers()
    else:
        headers = explicit.all_headers()
        if (cfg.token or cfg.headers) and not (args.token or args.header):
            print(f"[relay] 目标主机 {host_of(args.target)} 与配置的 {host_of(cfg.url) or '(无)'} 不同，未附带已保存的凭据。", file=sys.stderr)
    kwargs: dict[str, Any] = {}
    if args.json_body:
        try:
            kwargs["json"] = json.loads(args.json_body)
        except json.JSONDecodeError as e:
            raise SystemExit(f"--json-body 不是合法 JSON: {e}")
    elif args.data:
        kwargs["content"] = args.data.encode()
    async with httpx2.AsyncClient(timeout=cfg.timeout, headers=headers, follow_redirects=True) as hc:
        t0 = time.time()
        r = await hc.request(args.method.upper(), args.target, **kwargs)
        ms = round((time.time() - t0) * 1000)
    ctype = r.headers.get("content-type", "")
    if args.json:
        out: dict[str, Any] = {"status": r.status_code, "ms": ms, "headers": dict(r.headers)}
        try:
            out["body"] = r.json()
        except Exception:
            out["body"] = r.text
        print(json.dumps(out, ensure_ascii=False, indent=2))
    else:
        print(f"{args.method.upper()} {args.target} -> {r.status_code} ({ms} ms)  {ctype}")
        try:
            print(json.dumps(r.json(), ensure_ascii=False, indent=2) if "json" in ctype else r.text)
        except Exception:
            print(r.text)
    return 0 if r.is_success else 1


def cmd_config(cfg: Config, args: argparse.Namespace) -> int:
    if args.action == "show":
        print(f"配置文件: {CONFIG_PATH}{'' if os.path.exists(CONFIG_PATH) else '（不存在，使用内置默认值）'}")
        print(f"url          : {cfg.url}")
        print(f"token        : {mask(cfg.token)}")
        print(f"headers      : {json.dumps({k: mask(v) for k, v in cfg.headers.items()}, ensure_ascii=False)}")
        print(f"transport    : {cfg.transport}    legacy: {cfg.legacy}    keep_session: {cfg.keep_session}")
        print(f"mode         : {cfg.mode}    sock: {cfg.sock}    ping_interval: {cfg.ping_interval}s")
        print(f"timeout      : {cfg.timeout}s   connect_timeout: {cfg.connect_timeout}s")
        print(f"heartbeat    : timeout={cfg.ping_timeout}s  failures={cfg.ping_failures}  reconnect_max_delay={cfg.reconnect_max_delay}s")
        return 0
    if args.action == "set":
        save_config(cfg)
        print(f"已保存到 {CONFIG_PATH}")
        return cmd_config(cfg, argparse.Namespace(action="show"))
    if args.action == "clear":
        if os.path.exists(CONFIG_PATH):
            os.remove(CONFIG_PATH)
        print("配置已清除（恢复内置默认值）")
        return 0
    return 1


def daemon_cli_args(args: argparse.Namespace) -> list[str]:
    out: list[str] = []
    for opt, key in (("--url", "url"), ("--token", "token"), ("--transport", "transport"), ("--sock", "sock"),
                     ("--timeout", "timeout"), ("--connect-timeout", "connect_timeout"), ("--ping-interval", "ping_interval"),
                     ("--ping-timeout", "ping_timeout"), ("--ping-failures", "ping_failures"),
                     ("--reconnect-max-delay", "reconnect_max_delay")):
        v = getattr(args, key, None)
        if v is not None:
            out += [opt, str(v)]
    for h in getattr(args, "header", None) or []:
        out += ["-H", h]
    if getattr(args, "legacy", None) is not None:
        out.append("--legacy" if args.legacy else "--no-legacy")
    if getattr(args, "keep_session", False):
        out.append("--keep-session")
    return out


def cmd_daemon(cfg: Config, args: argparse.Namespace) -> int:
    action = args.action
    if action == "run":
        return asyncio.run(daemon_main(cfg))
    if action == "start":
        st = daemon_alive(cfg)
        if st:
            print("守护进程已在运行：")
            print_status(st)
            return 0
        log = open(DAEMON_LOG, "ab")
        subprocess.Popen([sys.executable, os.path.abspath(__file__), "daemon", "run", *daemon_cli_args(args)],
                         stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True, cwd=HERE)
        print(f"已启动守护进程，日志: {DAEMON_LOG}")
        deadline = time.time() + max(cfg.connect_timeout, 10) + 5
        st = None
        while time.time() < deadline:
            time.sleep(0.5)
            st = daemon_alive(cfg)
            if st and (st.get("connected") or st.get("lastError")):
                break
        if not st:
            print("守护进程没有响应，请查看日志。")
            return 1
        print_status(st)
        return 0 if st.get("connected") else 1
    if action == "status":
        st = daemon_alive(cfg)
        if not st:
            print(f"守护进程未运行（socket: {cfg.sock}）")
            return 1
        pj(st) if args.json else print_status(st)
        return 0 if st.get("connected") else 1
    if action in ("events", "reconnect", "stop"):
        if not daemon_alive(cfg):
            print("守护进程未运行")
            return 1
        r = daemon_request(cfg, "shutdown" if action == "stop" else action, {"n": args.n} if action == "events" else {}, timeout=20)
        if not r.get("ok"):
            print(f"失败: {r.get('error')}")
            return 1
        res = r.get("result")
        if args.json:
            pj(res)
        elif action == "events":
            for ev in res or []:
                print(f"{ev.get('at')}  {ev.get('kind')}  {ev.get('summary')}")
            if not res:
                print("(暂无服务端推送事件)")
        else:
            print(res if isinstance(res, str) else json.dumps(res, ensure_ascii=False))
        return 0
    if action == "log":
        if not os.path.exists(DAEMON_LOG):
            print("(还没有日志)")
            return 0
        with open(DAEMON_LOG, "r", encoding="utf-8", errors="replace") as f:
            print("".join(f.readlines()[-(args.n or 50):]), end="")
        return 0
    return 1


# ============================================================================
# 入口
# ============================================================================
def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--url")
    common.add_argument("--token", help="Bearer token；传空串表示不带")
    common.add_argument("-H", "--header", action="append", metavar="'K: V'")
    common.add_argument("--transport", choices=TRANSPORTS)
    common.add_argument("--mode", choices=MODES, help="daemon（默认，只用常驻会话）/ auto / direct")
    common.add_argument("--legacy", dest="legacy", action="store_true", default=None, help="直接 initialize 握手（旧服务兼容选项）")
    common.add_argument("--no-legacy", dest="legacy", action="store_false", help="先尝试新协议 server/discover")
    common.add_argument("--keep-session", dest="keep_session", action="store_true", help="direct 模式退出时不发 DELETE")
    common.add_argument("--timeout", type=float, help="请求读超时（秒，默认 60）")
    common.add_argument("--connect-timeout", dest="connect_timeout", type=float)
    common.add_argument("--sock")
    common.add_argument("--ping-interval", dest="ping_interval", type=float, help="心跳间隔（默认 25 秒，最小有效值 5 秒）")
    common.add_argument("--ping-timeout", type=float, help="单次心跳超时（默认 10 秒，独立于工具调用超时）")
    common.add_argument("--ping-failures", type=int, help="连续心跳失败多少次才重连（默认 3）")
    common.add_argument("--reconnect-max-delay", type=float, help="重连退避上限（默认 30 秒，持续重试直到停止）")
    common.add_argument("--json", action="store_true", help="以 JSON 输出")

    p = argparse.ArgumentParser(prog="relay.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sp = p.add_subparsers(dest="cmd", required=True)

    c = sp.add_parser("config", parents=[common], help="查看/保存配置")
    c.add_argument("action", choices=["show", "set", "clear"])

    d = sp.add_parser("daemon", parents=[common], help="常驻会话：start / status / events / reconnect / stop / log / run")
    d.add_argument("action", choices=["start", "status", "events", "reconnect", "stop", "log", "run"])
    d.add_argument("-n", type=int, default=50)

    sp.add_parser("probe", parents=[common], help="一次性诊断")
    sp.add_parser("info", parents=[common], help="服务端信息")
    sp.add_parser("tools", parents=[common], help="列出工具")
    sp.add_parser("ping", parents=[common], help="探活")
    call = sp.add_parser("call", parents=[common], help="调用工具")
    call.add_argument("tool")
    call.add_argument("--args", help="JSON 对象参数")
    call.add_argument("--args-file", dest="args_file", help="从文件读取 JSON 参数（大内容）")
    sp.add_parser("resources", parents=[common], help="列出资源")
    rd = sp.add_parser("read", parents=[common], help="读取资源")
    rd.add_argument("uri")
    sp.add_parser("prompts", parents=[common], help="列出 prompt")
    pr = sp.add_parser("prompt", parents=[common], help="获取 prompt")
    pr.add_argument("name")
    pr.add_argument("--args")
    h = sp.add_parser("http", parents=[common], help="通用 HTTP 请求")
    h.add_argument("method")
    h.add_argument("target")
    h.add_argument("--json-body", "--body", dest="json_body")
    h.add_argument("--data")
    return p


def main(argv: list[str] | None = None) -> int:
    try:
        signal.signal(signal.SIGPIPE, signal.SIG_DFL)
    except Exception:
        pass
    args = build_parser().parse_args(argv)
    cfg = apply_overrides(load_config(), args)
    if args.cmd == "config":
        return cmd_config(cfg, args)
    if args.cmd == "daemon":
        return cmd_daemon(cfg, args)
    table = {"probe": cmd_probe, "info": cmd_info, "tools": cmd_tools, "call": cmd_call, "resources": cmd_resources,
             "read": cmd_read, "prompts": cmd_prompts, "prompt": cmd_prompt, "ping": cmd_ping, "http": cmd_http}
    try:
        return asyncio.run(table[args.cmd](cfg, args))
    except ConnectFailed as e:
        print(str(e), file=sys.stderr)
        return 3
    except KeyboardInterrupt:
        return 130
    except MCPError as e:
        print(f"MCP 错误: {short_err(e)}", file=sys.stderr)
        return 4
    except Exception as e:  # noqa: BLE001
        print(f"失败: {short_err(e)}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
