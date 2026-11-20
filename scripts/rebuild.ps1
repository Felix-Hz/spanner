<#
.SYNOPSIS
  Rebuilds the release binary while spanner is installed.

.DESCRIPTION
  The recorder and any open window keep target\release\spanner.exe locked, so this stops both,
  builds, and starts the recorder again. Asks for administrator approval once (the recorder runs elevated).

  pwsh scripts/rebuild.ps1 [-Open]
#>
param([switch]$Open, [switch]$Elevated, [string]$Log)

$repo = Split-Path $PSScriptRoot -Parent

if (-not $Elevated) {
  $log = Join-Path ([IO.Path]::GetTempPath()) "spanner-rebuild.log"
  Remove-Item $log -ErrorAction SilentlyContinue
  $argList = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Elevated -Log `"$log`""
  $p = Start-Process pwsh -ArgumentList $argList -Verb RunAs -Wait -PassThru -WindowStyle Hidden
  Get-Content $log -ErrorAction SilentlyContinue
  if ($p.ExitCode -ne 0) { exit $p.ExitCode }
  if ($Open) { Start-Process (Join-Path $repo "target\release\spanner.exe") }
  exit 0
}

& {
  schtasks /End /TN "\spanner\recorder" 2>&1 | Out-Null
  $running = Get-Process spanner -ErrorAction SilentlyContinue
  $running | Stop-Process -Force
  $running | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
  Push-Location $repo
  cargo build --release 2>&1 | Select-Object -Last 3
  $code = $LASTEXITCODE
  Pop-Location
  schtasks /Run /TN "\spanner\recorder" 2>&1 | Out-Null
  "build exit code: $code"
  exit $code
} *>&1 | Out-File $Log -Encoding utf8
exit $LASTEXITCODE
