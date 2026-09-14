export const RESTART_SUPERVISED_FLAG = '--restart-supervised';
export const RESTART_SUPERVISOR_ENV = 'CTMCP_RESTART_SUPERVISOR';
export const RESTART_SUPERVISOR_VALUE = 'active-v1';

export function restartSupervisedFromArgv(
  argv: readonly string[],
  env: Readonly<Record<string, string | undefined>> = process.env
): boolean {
  return argv.includes(RESTART_SUPERVISED_FLAG)
    && env[RESTART_SUPERVISOR_ENV] === RESTART_SUPERVISOR_VALUE;
}
