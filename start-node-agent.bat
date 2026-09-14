@echo off
setlocal EnableExtensions
chcp 65001 >nul
title Coding Tools MCP Node Agent

for %%I in ("%~dp0.") do set "REPO_ROOT=%%~fI"
set "AGENT_DIR=%REPO_ROOT%\packages\node-agent"
set "FORCE_BUILD=0"

if /I "%~1"=="--rebuild" (
  set "FORCE_BUILD=1"
  shift
)

echo [Coding Tools MCP] Node Agent quick start
echo Worktree: %REPO_ROOT%
echo.

if not exist "%AGENT_DIR%\package.json" (
  echo ERROR: Node Agent package was not found:
  echo   %AGENT_DIR%\package.json
  goto :failed
)

where node >nul 2>nul
if errorlevel 1 (
  echo ERROR: Node.js was not found in PATH.
  echo Install Node.js 22 or later, then run this file again.
  goto :failed
)

where pnpm >nul 2>nul
if errorlevel 1 (
  echo ERROR: pnpm was not found in PATH.
  echo Install pnpm 12.0.0 or newer, then run this file again.
  goto :failed
)

set "NODE_MAJOR="
for /f "tokens=1 delims=." %%V in ('node -p "process.versions.node" 2^>nul') do set "NODE_MAJOR=%%V"
if not defined NODE_MAJOR (
  echo ERROR: Unable to determine the Node.js version.
  goto :failed
)
if %NODE_MAJOR% LSS 22 (
  echo ERROR: Node.js 22 or later is required. Current version:
  node --version
  goto :failed
)

cd /d "%REPO_ROOT%"
if errorlevel 1 (
  echo ERROR: Unable to enter the repository root.
  goto :failed
)

if not exist "%AGENT_DIR%\node_modules\@typescript\native\bin\tsc" (
  echo Installing pnpm workspace dependencies...
  call pnpm install --frozen-lockfile
  if errorlevel 1 (
    echo ERROR: pnpm install failed.
    goto :failed
  )
)

if not exist "dist\cli.js" set "FORCE_BUILD=1"
if not exist "dist\ui\app.js" set "FORCE_BUILD=1"

if "%FORCE_BUILD%"=="1" (
  echo Building Node Agent...
  call pnpm --filter @coding-tools/node-agent run build
  if errorlevel 1 (
    echo ERROR: Node Agent build failed.
    goto :failed
  )
)

echo.
echo Starting Coding Tools MCP Node Agent...
echo Each Workspace uses its own saved Port, OAuth, tunnel, policy, and folders.
echo The Agent will print every MCP endpoint and the primary Management UI URL.
echo Press Ctrl+C to stop.
echo.

set "CTMCP_RESTART_SUPERVISOR=active-v1"

cd /d "%REPO_ROOT%"
if errorlevel 1 (
  echo ERROR: Unable to enter the repository root.
  goto :failed
)

:run_agent
node "%AGENT_DIR%\dist\cli.js" --restart-supervised %*
set "EXIT_CODE=%ERRORLEVEL%"
if "%EXIT_CODE%"=="75" (
  echo.
  echo Restart requested from Web UI. Starting Node Agent again...
  ping 127.0.0.1 -n 2 >nul
  goto :run_agent
)
if not "%EXIT_CODE%"=="0" (
  echo.
  echo Node Agent exited with code %EXIT_CODE%.
  pause
)
exit /b %EXIT_CODE%

:failed
echo.
pause
exit /b 1
