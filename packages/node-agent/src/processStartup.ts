import type { ChildProcessWithoutNullStreams } from 'node:child_process';

export const WINDOWS_STARTUP_SLOTS = 4;
export const WINDOWS_START_INTERVAL_MS = 75;
export const WINDOWS_STARTUP_PROBE_MS = 125;
export const WINDOWS_FAILURE_WINDOW_MS = 10_000;
export const WINDOWS_CIRCUIT_BREAKER_THRESHOLD = 3;
export const WINDOWS_CIRCUIT_BREAKER_DELAY_MS = 3_000;
export const WINDOWS_RETRY_DELAYS_MS = [250, 750, 1_500] as const;
export const WINDOWS_DLL_INIT_FAILED_UNSIGNED = 0xc0000142;
export const WINDOWS_DLL_INIT_FAILED_SIGNED = WINDOWS_DLL_INIT_FAILED_UNSIGNED | 0;

export interface StartupDiagnostics {
  attempts: number;
  gateWaitMs: number;
  retryDelaysMs: number[];
  errorDialogSuppressed: boolean;
  startupSlots: number;
  startIntervalMs: number;
}

export interface StartupClock {
  now(): number;
  sleep(ms: number, signal?: AbortSignal): Promise<void>;
}

export interface StartedChild {
  child: ChildProcessWithoutNullStreams;
  diagnostics: StartupDiagnostics;
  earlyExit: boolean;
  handoff(): Error[];
}

export type ProcessStartupFailure = 'spawn' | 'loader_initialization' | 'cancelled' | 'timeout';

export class ProcessStartupError extends Error {
  readonly kind: ProcessStartupFailure;
  readonly diagnostics: StartupDiagnostics;
  readonly exitCode?: number;
  readonly source?: unknown;

  constructor(
    kind: ProcessStartupFailure,
    message: string,
    diagnostics: StartupDiagnostics,
    options: { exitCode?: number; source?: unknown } = {}
  ) {
    super(message);
    this.name = 'ProcessStartupError';
    this.kind = kind;
    this.diagnostics = diagnostics;
    this.exitCode = options.exitCode;
    this.source = options.source;
  }
}

export interface StartupControllerOptions {
  platform?: NodeJS.Platform;
  clock?: StartupClock;
  startupSlots?: number;
  startIntervalMs?: number;
  probeMs?: number;
  failureWindowMs?: number;
  circuitBreakerThreshold?: number;
  circuitBreakerDelayMs?: number;
  retryDelaysMs?: readonly number[];
}

export interface StartOptions {
  signal?: AbortSignal;
  deadlineMs?: number;
  terminate?: (child: ChildProcessWithoutNullStreams) => void | Promise<void>;
}

interface Waiter {
  resolve: (release: () => void) => void;
  reject: (error: Error) => void;
  signal?: AbortSignal;
  onAbort?: () => void;
  timer?: NodeJS.Timeout;
}

class StartupSlotPool {
  readonly limit: number;
  #active = 0;
  #waiters: Waiter[] = [];

  constructor(limit: number) {
    this.limit = Math.max(1, Math.trunc(limit));
  }

  async acquire(signal?: AbortSignal, timeoutMs = 0): Promise<() => void> {
    if (signal?.aborted) throw waitError('cancelled');
    if (this.#active < this.limit) return this.#grant();
    return new Promise<() => void>((resolve, reject) => {
      const waiter: Waiter = { resolve, reject, signal };
      const remove = () => {
        const index = this.#waiters.indexOf(waiter);
        if (index >= 0) this.#waiters.splice(index, 1);
      };
      waiter.onAbort = () => {
        remove();
        this.#cleanup(waiter);
        reject(waitError('cancelled'));
      };
      if (timeoutMs > 0) {
        waiter.timer = setTimeout(() => {
          remove();
          this.#cleanup(waiter);
          reject(waitError('timeout'));
        }, timeoutMs);
        waiter.timer.unref();
      }
      signal?.addEventListener('abort', waiter.onAbort, { once: true });
      this.#waiters.push(waiter);
      if (signal?.aborted) waiter.onAbort();
    });
  }

  #cleanup(waiter: Waiter): void {
    if (waiter.timer) clearTimeout(waiter.timer);
    if (waiter.onAbort) waiter.signal?.removeEventListener('abort', waiter.onAbort);
  }

  #grant(): () => void {
    this.#active += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.#active -= 1;
      while (this.#waiters.length) {
        const waiter = this.#waiters.shift()!;
        this.#cleanup(waiter);
        if (waiter.signal?.aborted) continue;
        waiter.resolve(this.#grant());
        break;
      }
    };
  }
}

class StartupWaitError extends Error {
  constructor(readonly kind: 'cancelled' | 'timeout') {
    super(kind === 'cancelled' ? 'process startup cancelled' : 'process startup timed out');
    this.name = 'StartupWaitError';
  }
}

function waitError(kind: StartupWaitError['kind']): StartupWaitError {
  return new StartupWaitError(kind);
}

const systemClock: StartupClock = {
  now: () => Date.now(),
  sleep: (ms, signal) => new Promise<void>((resolve, reject) => {
    if (signal?.aborted) { reject(waitError('cancelled')); return; }
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', onAbort);
      if (error) reject(error); else resolve();
    };
    const onAbort = () => finish(waitError('cancelled'));
    const timer = setTimeout(() => finish(), Math.max(0, ms));
    timer.unref();
    signal?.addEventListener('abort', onAbort, { once: true });
  })
};

function emptyDiagnostics(platform: NodeJS.Platform, slots: number, intervalMs: number): StartupDiagnostics {
  const windows = platform === 'win32';
  return {
    attempts: 0,
    gateWaitMs: 0,
    retryDelaysMs: [],
    errorDialogSuppressed: false,
    startupSlots: windows ? slots : 0,
    startIntervalMs: windows ? intervalMs : 0
  };
}

export function startupDiagnosticsJson(diagnostics: StartupDiagnostics): Record<string, unknown> {
  return {
    attempts: diagnostics.attempts,
    retry_count: Math.max(0, diagnostics.attempts - 1),
    gate_wait_ms: Math.max(0, Math.trunc(diagnostics.gateWaitMs)),
    retry_delays_ms: diagnostics.retryDelaysMs.map(value => Math.max(0, Math.trunc(value))),
    error_dialog_suppressed: diagnostics.errorDialogSuppressed,
    startup_slots: diagnostics.startupSlots,
    start_interval_ms: diagnostics.startIntervalMs
  };
}

export function isWindowsLoaderInitializationFailure(exitCode: number | null | undefined): boolean {
  return exitCode === WINDOWS_DLL_INIT_FAILED_UNSIGNED || exitCode === WINDOWS_DLL_INIT_FAILED_SIGNED;
}

export class ProcessStartupController {
  readonly platform: NodeJS.Platform;
  readonly clock: StartupClock;
  readonly startupSlots: number;
  readonly startIntervalMs: number;
  readonly probeMs: number;
  readonly failureWindowMs: number;
  readonly circuitBreakerThreshold: number;
  readonly circuitBreakerDelayMs: number;
  readonly retryDelaysMs: readonly number[];

  #slots: StartupSlotPool;
  #nextStartMs: number;
  #loaderFailures: number[] = [];
  #circuitOpenUntilMs = 0;
  #jitterSequence = 0;

  constructor(options: StartupControllerOptions = {}) {
    this.platform = options.platform ?? process.platform;
    this.clock = options.clock ?? systemClock;
    this.startupSlots = Math.max(1, Math.trunc(options.startupSlots ?? WINDOWS_STARTUP_SLOTS));
    this.startIntervalMs = Math.max(0, Math.trunc(options.startIntervalMs ?? WINDOWS_START_INTERVAL_MS));
    this.probeMs = Math.max(0, Math.trunc(options.probeMs ?? WINDOWS_STARTUP_PROBE_MS));
    this.failureWindowMs = Math.max(0, Math.trunc(options.failureWindowMs ?? WINDOWS_FAILURE_WINDOW_MS));
    this.circuitBreakerThreshold = Math.max(1, Math.trunc(options.circuitBreakerThreshold ?? WINDOWS_CIRCUIT_BREAKER_THRESHOLD));
    this.circuitBreakerDelayMs = Math.max(0, Math.trunc(options.circuitBreakerDelayMs ?? WINDOWS_CIRCUIT_BREAKER_DELAY_MS));
    this.retryDelaysMs = options.retryDelaysMs ?? WINDOWS_RETRY_DELAYS_MS;
    this.#slots = new StartupSlotPool(this.startupSlots);
    this.#nextStartMs = this.clock.now();
  }

  async start(
    launch: () => ChildProcessWithoutNullStreams,
    options: StartOptions = {}
  ): Promise<StartedChild> {
    const diagnostics = emptyDiagnostics(this.platform, this.startupSlots, this.startIntervalMs);
    if (this.platform !== 'win32') {
      diagnostics.attempts = 1;
      return this.#launchAttempt(launch, diagnostics, options);
    }

    while (true) {
      const gateStarted = this.clock.now();
      const remaining = this.#remaining(options.deadlineMs);
      let release: (() => void) | undefined;
      let attempt: StartedChild | undefined;
      try {
        this.#throwIfStopped(options.signal, options.deadlineMs);
        release = await this.#slots.acquire(options.signal, remaining);
        await this.#reserveStart(options.signal, options.deadlineMs);
        diagnostics.gateWaitMs += Math.max(0, this.clock.now() - gateStarted);
        diagnostics.attempts += 1;
        attempt = await this.#launchAttempt(launch, diagnostics, options);
        await this.#sleepWithin(this.probeMs, options.signal);
      } catch (error) {
        if (attempt) await this.#discard(attempt, options.terminate, true);
        release?.();
        throw this.#normalizeError(error, diagnostics);
      }
      release();

      const loaderFailed = isWindowsLoaderInitializationFailure(attempt.child.exitCode);
      if (!loaderFailed) {
        return {
          ...attempt,
          diagnostics: { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
          earlyExit: attempt.child.exitCode !== null || attempt.child.signalCode !== null
        };
      }

      await this.#discard(attempt, options.terminate, false);
      this.#recordLoaderFailure();
      const retryIndex = diagnostics.attempts - 1;
      if (retryIndex >= this.retryDelaysMs.length) {
        throw new ProcessStartupError(
          'loader_initialization',
          'Windows could not initialize the child process (0xc0000142) after retries',
          { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
          { exitCode: WINDOWS_DLL_INIT_FAILED_SIGNED }
        );
      }
      const delay = this.#retryDelay(retryIndex);
      diagnostics.retryDelaysMs.push(delay);
      try {
        await this.#sleepWithin(delay, options.signal, options.deadlineMs);
      } catch (error) {
        throw this.#normalizeError(error, diagnostics);
      }
    }
  }

  #remaining(deadlineMs?: number): number {
    if (deadlineMs === undefined) return 0;
    return Math.max(0, Math.trunc(deadlineMs - this.clock.now()));
  }

  async #reserveStart(signal?: AbortSignal, deadlineMs?: number): Promise<void> {
    while (true) {
      this.#throwIfStopped(signal, deadlineMs);
      const now = this.clock.now();
      this.#pruneFailures(now);
      const readyAt = Math.max(this.#nextStartMs, this.#circuitOpenUntilMs || now);
      if (readyAt <= now) {
        this.#nextStartMs = now + this.startIntervalMs;
        if (this.#circuitOpenUntilMs <= now) this.#circuitOpenUntilMs = 0;
        return;
      }
      await this.#sleepWithin(readyAt - now, signal, deadlineMs);
    }
  }

  async #launchAttempt(
    launch: () => ChildProcessWithoutNullStreams,
    diagnostics: StartupDiagnostics,
    options: StartOptions
  ): Promise<StartedChild> {
    this.#throwIfStopped(options.signal, options.deadlineMs);
    let child: ChildProcessWithoutNullStreams;
    try {
      child = launch();
    } catch (error) {
      throw new ProcessStartupError(
        'spawn',
        error instanceof Error ? error.message : String(error),
        { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
        { source: error }
      );
    }

    const errors: Error[] = [];
    let handedOff = false;
    const onError = (error: Error) => { errors.push(error); };
    child.on('error', onError);
    const handoff = () => {
      if (handedOff) return [];
      handedOff = true;
      child.off('error', onError);
      return errors.splice(0);
    };

    try {
      await this.#waitForSpawn(child, options.signal, options.deadlineMs);
    } catch (error) {
      handoff();
      await this.#terminate(child, options.terminate);
      throw this.#normalizeError(error, diagnostics);
    }

    return {
      child,
      diagnostics: { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
      earlyExit: false,
      handoff
    };
  }

  #waitForSpawn(
    child: ChildProcessWithoutNullStreams,
    signal?: AbortSignal,
    deadlineMs?: number
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      let settled = false;
      let timer: NodeJS.Timeout | undefined;
      const cleanup = () => {
        child.off('spawn', onSpawn);
        child.off('error', onSpawnError);
        signal?.removeEventListener('abort', onAbort);
        if (timer) clearTimeout(timer);
      };
      const finish = (error?: Error) => {
        if (settled) return;
        settled = true;
        cleanup();
        if (error) reject(error); else resolve();
      };
      const onSpawn = () => finish();
      const onSpawnError = (source: Error) => finish(new ProcessStartupError(
        'spawn',
        source.message,
        emptyDiagnostics(this.platform, this.startupSlots, this.startIntervalMs),
        { source }
      ));
      const onAbort = () => finish(waitError('cancelled'));
      child.once('spawn', onSpawn);
      child.once('error', onSpawnError);
      signal?.addEventListener('abort', onAbort, { once: true });
      const remaining = deadlineMs === undefined ? 0 : Math.max(0, deadlineMs - this.clock.now());
      if (deadlineMs !== undefined) {
        if (remaining <= 0) { finish(waitError('timeout')); return; }
        timer = setTimeout(() => finish(waitError('timeout')), remaining);
        timer.unref();
      }
      if (signal?.aborted) onAbort();
    });
  }

  async #sleepWithin(ms: number, signal?: AbortSignal, deadlineMs?: number): Promise<void> {
    this.#throwIfStopped(signal, deadlineMs);
    if (ms <= 0) return;
    const remaining = deadlineMs === undefined ? ms : Math.max(0, deadlineMs - this.clock.now());
    if (remaining <= 0) throw waitError('timeout');
    const delay = Math.min(ms, remaining);
    await this.clock.sleep(delay, signal);
    if (delay < ms) throw waitError('timeout');
    this.#throwIfStopped(signal, deadlineMs);
  }

  #throwIfStopped(signal?: AbortSignal, deadlineMs?: number): void {
    if (signal?.aborted) throw waitError('cancelled');
    if (deadlineMs !== undefined && this.clock.now() >= deadlineMs) throw waitError('timeout');
  }

  #pruneFailures(now: number): void {
    this.#loaderFailures = this.#loaderFailures.filter(failure => now - failure <= this.failureWindowMs);
  }

  #recordLoaderFailure(): void {
    const now = this.clock.now();
    this.#pruneFailures(now);
    this.#loaderFailures.push(now);
    if (this.#loaderFailures.length >= this.circuitBreakerThreshold) {
      this.#circuitOpenUntilMs = Math.max(this.#circuitOpenUntilMs, now + this.circuitBreakerDelayMs);
    }
  }

  #retryDelay(retryIndex: number): number {
    const base = this.retryDelaysMs[retryIndex] ?? 0;
    const jitter = (this.#jitterSequence * 17) % 51;
    this.#jitterSequence += 1;
    return base + jitter;
  }

  async #discard(
    started: StartedChild,
    terminate: StartOptions['terminate'],
    shouldTerminate: boolean
  ): Promise<void> {
    started.handoff();
    started.child.stdout.resume();
    started.child.stderr.resume();
    if (shouldTerminate && started.child.exitCode === null && started.child.signalCode === null) {
      await this.#terminate(started.child, terminate);
    }
  }

  async #terminate(
    child: ChildProcessWithoutNullStreams,
    terminate?: StartOptions['terminate']
  ): Promise<void> {
    try {
      if (terminate) await terminate(child);
      else child.kill('SIGKILL');
    } catch {
      try { child.kill('SIGKILL'); } catch { /* best effort */ }
    }
  }

  #normalizeError(error: unknown, diagnostics: StartupDiagnostics): ProcessStartupError {
    if (error instanceof ProcessStartupError) {
      if (error.diagnostics.attempts > 0) return error;
      return new ProcessStartupError(error.kind, error.message, {
        ...diagnostics,
        retryDelaysMs: [...diagnostics.retryDelaysMs]
      }, { exitCode: error.exitCode, source: error.source });
    }
    if (error instanceof StartupWaitError) {
      return new ProcessStartupError(
        error.kind,
        error.message,
        { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
        { source: error }
      );
    }
    return new ProcessStartupError(
      'spawn',
      error instanceof Error ? error.message : String(error),
      { ...diagnostics, retryDelaysMs: [...diagnostics.retryDelaysMs] },
      { source: error }
    );
  }
}

export const processStartupController = new ProcessStartupController();
