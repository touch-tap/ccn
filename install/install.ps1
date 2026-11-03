# Installs ccn, the Claude Code Notifier CLI, on Windows.
#
#   irm https://claudecodenotifier.com/install.ps1 | iex; ccn pair 482913
#   irm https://claudecodenotifier.com/install.ps1 | iex; ccn activate CCN-XXXX-XXXX-XXXX-XXXX; ccn pair 482913
#
# $env:CCN_VERSION = "0.2.0" picks a release (default: the latest).
# $env:CCN_INSTALL_DIR overrides %LOCALAPPDATA%\Programs\ccn.
# $env:CCN_NO_ANALYTICS = "1" (or DO_NOT_TRACK) stops the anonymous install events.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Where the install line came from: web, app or link when the script is fetched from
# claudecodenotifier.com/<source>/install.ps1 (the site rewrites the next line), direct otherwise.
$sourceTag = if ($env:CCN_INSTALL_SOURCE) { $env:CCN_INSTALL_SOURCE } else { 'direct' }
# The same rule as ccn's telemetry.rs: lowercase letters, digits, _ and -, at most 32 of them
# (-cnotmatch: PowerShell's -notmatch ignores case)
if ($sourceTag -cnotmatch '^[a-z0-9_-]{1,32}$') { $sourceTag = 'other' }

# One anonymous event per step, so we can see where installs fail: the event name, the stage, the
# processor. Nothing else. $env:CCN_NO_ANALYTICS = "1" (or DO_NOT_TRACK) turns it off.
$stage = 'start'
function Send-SetupEvent([string]$name, [hashtable]$properties = @{}) {
    if (($env:CCN_NO_ANALYTICS -and $env:CCN_NO_ANALYTICS -ne '0') -or ($env:DO_NOT_TRACK -and $env:DO_NOT_TRACK -ne '0')) { return }
    $properties['method'] = 'powershell'
    $properties['source'] = $sourceTag
    $body = @{
        context = @{ platform = 'cli'; deviceModel = "windows-$arch"; environment = 'production' }
        events = @(@{ name = $name; properties = $properties })
    } | ConvertTo-Json -Depth 5 -Compress
    $server = if ($env:CCN_SERVER_URL) { $env:CCN_SERVER_URL } else { 'https://api.claudecodenotifier.com' }
    try { Invoke-RestMethod -Method Post -Uri "$server/api/events" -ContentType 'application/json' -Body $body -TimeoutSec 3 | Out-Null } catch { }
}

$repo = 'touch-tap/ccn'
$dir = if ($env:CCN_INSTALL_DIR) { $env:CCN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\ccn' }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$asset = "ccn-$arch-pc-windows-msvc.zip"
$base = if ($env:CCN_DOWNLOAD_BASE) { $env:CCN_DOWNLOAD_BASE } elseif ($env:CCN_VERSION) { "https://github.com/$repo/releases/download/v$($env:CCN_VERSION.TrimStart('v'))" } else { "https://github.com/$repo/releases/latest/download" }

Send-SetupEvent 'cli_install_started'

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ccn-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $stage = 'download'
    Write-Host "Downloading ccn for $arch Windows..."
    Invoke-WebRequest "$base/$asset" -OutFile (Join-Path $tmp $asset) -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing

    $stage = 'checksum'
    $expected = (Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | ForEach-Object { ($_ -split '\s+')[0] }) | Select-Object -First 1
    $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected.ToLower() -ne $actual) { throw "checksum mismatch for $asset" }

    $stage = 'unpack'
    Expand-Archive (Join-Path $tmp $asset) -DestinationPath $tmp -Force
    # Run the download before it replaces anything, so one that doesn't run leaves the old ccn
    $stage = 'run'
    $new = Join-Path $tmp 'ccn.exe'
    $version = & $new --version
    if ($LASTEXITCODE -ne 0 -or -not $version) { throw "the downloaded ccn did not run, so nothing was changed" }

    $stage = 'install'
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    # A running ccn.exe cannot be overwritten but can be renamed, so move it aside first, and put
    # it back if the new one cannot take its place
    $target = Join-Path $dir 'ccn.exe'
    $old = Join-Path $dir 'ccn.old.exe'
    $movedAside = $false
    if (Test-Path $target) {
        Remove-Item $old -Force -ErrorAction SilentlyContinue
        Move-Item $target $old -Force
        $movedAside = $true
    }
    try {
        Move-Item $new $target
    } catch {
        if ($movedAside) { Move-Item $old $target -Force -ErrorAction SilentlyContinue }
        throw
    }
    Write-Host "Installed $version to $target"
} catch {
    Send-SetupEvent 'cli_install_failed' @{ stage = $stage }
    throw
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# On PATH for this session (so `ccn pair` works on the same line) and for new terminals
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" } else { '' }) + $dir), 'User')
}
if (-not (($env:Path -split ';') -contains $dir)) { $env:Path = "$env:Path;$dir" }

Send-SetupEvent 'cli_install_finished'
# The install line runs `ccn pair` next in this same session
$env:CCN_INSTALL_METHOD = 'powershell'
$env:CCN_INSTALL_SOURCE = $sourceTag

# Hooks and desktop notifications first, before any pairing: from here on every claude session is
# covered, phone or no phone. A problem with settings.json doesn't stop the install.
Write-Host ""
try { & $target setup } catch { Write-Host "The hooks weren't installed: $_`nFix that, then run: ccn setup" }

Write-Host ""
Write-Host "Hooks are installed, so you already get desktop notifications on this computer."
Write-Host "Next: run ccn and scan the QR code with your phone, or ccn pair <the code on your phone>"
