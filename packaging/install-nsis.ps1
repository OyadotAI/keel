# Puts makensis on PATH for the Windows installer tests, without depending on Chocolatey.
#
# Both workflows used `choco install nsis`, and when Chocolatey's package feed was unreachable
# (2026-11-24) the installer test failed and the 0.4.91 release was never published. NSIS now
# comes from the same pinned archive Tauri's own bundler downloads, checked against its SHA-256.
# An NSIS that is already there is used as it is: the system install, or the copy Tauri has
# cached after a build.
$ErrorActionPreference = 'Stop'
$Url = 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip'
$Sha256 = 'c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1'
$Attempts = 4

function Use-Nsis([string]$Directory) {
    $env:PATH = "$Directory;$env:PATH"
    if ($env:GITHUB_PATH) { Add-Content -Path $env:GITHUB_PATH -Value $Directory }
    Write-Host "NSIS: $Directory"
}

if (Get-Command makensis -ErrorAction SilentlyContinue) { Write-Host 'NSIS: already on PATH'; return }
foreach ($known in @((Join-Path ${env:ProgramFiles(x86)} 'NSIS'), (Join-Path $env:LOCALAPPDATA 'tauri\NSIS'))) {
    if (Test-Path (Join-Path $known 'makensis.exe')) { Use-Nsis $known; return }
}

$work = Join-Path ([IO.Path]::GetTempPath()) 'oya-factory-nsis'
New-Item -ItemType Directory -Force -Path $work | Out-Null
$zip = Join-Path $work 'nsis.zip'
for ($try = 1; $try -le $Attempts; $try++) {
    try { Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing; break }
    catch { if ($try -eq $Attempts) { throw "Could not download NSIS from $Url after $Attempts tries: $_" }; Start-Sleep -Seconds (5 * $try) }
}
$actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
if ($actual -ne $Sha256) { throw "NSIS archive checksum mismatch: expected $Sha256, got $actual" }
Expand-Archive -Path $zip -DestinationPath $work -Force
$found = Get-ChildItem -Path $work -Recurse -Filter makensis.exe | Sort-Object { $_.FullName.Length } | Select-Object -First 1
if (-not $found) { throw 'The NSIS archive holds no makensis.exe' }
Use-Nsis $found.DirectoryName
