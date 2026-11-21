<#
.SYNOPSIS
  Rebuilds the release binary while spanner is installed.

.DESCRIPTION
  The recorder and any open window keep target\release\spanner.exe locked, and the recorder's task restarts
  it a minute after it is stopped. So this builds into target\stage with nothing stopped, then stops both,
  swaps the exe in and starts the recorder again, which takes seconds. Asks for administrator approval once,
  for the swap (the recorder runs elevated).

  pwsh scripts/rebuild.ps1 [-Open]
#>
param([switch]$Open, [switch]$Elevated, [string]$Log)

$repo = Split-Path $PSScriptRoot -Parent
$staged = Join-Path $repo "target\stage\release\spanner.exe"
$installed = Join-Path $repo "target\release\spanner.exe"

if (-not $Elevated) {
  Push-Location $repo
  cargo build --release --target-dir target\stage 2>&1 | Select-Object -Last 3
  $code = $LASTEXITCODE
  Pop-Location
  "build exit code: $code"
  if ($code -ne 0) { exit $code }

  $log = Join-Path ([IO.Path]::GetTempPath()) "spanner-rebuild.log"
  Remove-Item $log -ErrorAction SilentlyContinue
  $argList = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Elevated -Log `"$log`""
  $p = Start-Process pwsh -ArgumentList $argList -Verb RunAs -Wait -PassThru -WindowStyle Hidden
  Get-Content $log -ErrorAction SilentlyContinue
  if ($p.ExitCode -ne 0) { exit $p.ExitCode }
  if ($Open) { Start-Process $installed }
  exit 0
}

& {
  schtasks /End /TN "\spanner\recorder" 2>&1 | Out-Null
  $running = Get-Process spanner -ErrorAction SilentlyContinue
  $running | Stop-Process -Force
  $running | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
  $code = 0
  # A running exe can't be overwritten but can be renamed, so the old one moves aside whatever still holds it
  $old = "$installed.old"
  Remove-Item $old -Force -ErrorAction SilentlyContinue
  try {
    if (Test-Path $installed) { Move-Item $installed $old -Force -ErrorAction Stop }
    Copy-Item $staged $installed -Force -ErrorAction Stop
    "swapped in $installed"
  }
  catch { "swap failed: $($_.Exception.Message)"; $code = 1 }
  schtasks /Run /TN "\spanner\recorder" 2>&1 | Out-Null
  exit $code
} *>&1 | Out-File $Log -Encoding utf8
exit $LASTEXITCODE
