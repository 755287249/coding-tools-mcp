/** Minimal unified-diff parser used by the task panel's diff viewer. */

export type DiffLineType = "ctx" | "add" | "del" | "hunk" | "meta";

export interface DiffLine {
  type: DiffLineType;
  text: string;
  oldNo?: number;
  newNo?: number;
}

export interface DiffFile {
  path: string;
  oldPath: string;
  newPath: string;
  status: "add" | "delete" | "update";
  lines: DiffLine[];
  added: number;
  removed: number;
}

export interface SplitRow {
  hunk?: string;
  left?: DiffLine;
  right?: DiffLine;
}

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/;

function stripPrefix(path: string, prefix: string): string {
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

/**
 * Parses a (multi-file) unified diff. Hunk line counts are honoured so removed
 * lines that themselves start with "-- " are never mistaken for file headers.
 */
export function parseUnifiedDiff(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  const lines = diff.replace(/\r\n/g, "\n").split("\n");
  let file: DiffFile | null = null;
  let oldLeft = 0;
  let newLeft = 0;
  let oldNo = 0;
  let newNo = 0;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const inHunk = file !== null && (oldLeft > 0 || newLeft > 0);

    // A new file header normally only appears outside a hunk; the extra check tolerates
    // producers whose hunk counts are off by recognising "--- / +++ / @@" triples.
    const isHeader =
      line.startsWith("--- ") &&
      lines[i + 1]?.startsWith("+++ ") &&
      (!inHunk || lines[i + 2]?.startsWith("@@ "));
    if (isHeader) {
      oldLeft = 0;
      newLeft = 0;
      const oldRaw = line.slice(4).trim();
      const newRaw = lines[i + 1].slice(4).trim();
      const oldPath = stripPrefix(oldRaw, "a/");
      const newPath = stripPrefix(newRaw, "b/");
      const status = oldRaw === "/dev/null" ? "add" : newRaw === "/dev/null" ? "delete" : "update";
      file = {
        path: status === "delete" ? oldPath : newPath,
        oldPath,
        newPath,
        status,
        lines: [],
        added: 0,
        removed: 0,
      };
      files.push(file);
      i += 1;
      continue;
    }
    if (!file) continue;

    const hunk = !inHunk ? HUNK_RE.exec(line) : null;
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[3]);
      oldLeft = hunk[2] === undefined ? 1 : Number(hunk[2]);
      newLeft = hunk[4] === undefined ? 1 : Number(hunk[4]);
      file.lines.push({ type: "hunk", text: line });
      continue;
    }
    if (line.startsWith("\\")) {
      file.lines.push({ type: "meta", text: line.slice(1).trim() });
      continue;
    }
    if (!inHunk) continue;

    const marker = line[0];
    const text = line.slice(1);
    if (marker === "+") {
      file.lines.push({ type: "add", text, newNo });
      file.added += 1;
      newNo += 1;
      newLeft -= 1;
    } else if (marker === "-") {
      file.lines.push({ type: "del", text, oldNo });
      file.removed += 1;
      oldNo += 1;
      oldLeft -= 1;
    } else {
      // Context line (a leading space, or an empty line from trimmed output).
      file.lines.push({ type: "ctx", text: marker === " " ? text : line, oldNo, newNo });
      oldNo += 1;
      newNo += 1;
      oldLeft -= 1;
      newLeft -= 1;
    }
  }
  return files;
}

/** Pairs removed/added runs so they render side by side. */
export function toSplitRows(lines: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];
  const flush = () => {
    const count = Math.max(dels.length, adds.length);
    for (let i = 0; i < count; i += 1) rows.push({ left: dels[i], right: adds[i] });
    dels = [];
    adds = [];
  };
  for (const line of lines) {
    if (line.type === "del") {
      if (adds.length) flush();
      dels.push(line);
    } else if (line.type === "add") {
      adds.push(line);
    } else {
      flush();
      if (line.type === "hunk") rows.push({ hunk: line.text });
      else if (line.type === "ctx") rows.push({ left: line, right: line });
    }
  }
  flush();
  return rows;
}

export function formatBytes(bytes: number | undefined | null): string {
  if (bytes === undefined || bytes === null || !Number.isFinite(bytes)) return "–";
  const abs = Math.abs(bytes);
  if (abs < 1024) return `${bytes} B`;
  if (abs < 1024 * 1024) return `${(bytes / 1024).toFixed(abs < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatDelta(before?: number, after?: number): string {
  if (before === undefined || after === undefined) return "";
  const delta = after - before;
  if (delta === 0) return "±0 B";
  return `${delta > 0 ? "+" : "−"}${formatBytes(Math.abs(delta))}`;
}

export function formatDuration(ms: number | undefined | null): string {
  if (ms === undefined || ms === null || !Number.isFinite(ms)) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

export function formatClock(ts: number): string {
  const date = new Date(ts);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** Last path segment, for compact rows. */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index >= 0 ? trimmed.slice(index + 1) || trimmed : trimmed;
}

export function dirName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index > 0 ? trimmed.slice(0, index) : "";
}
