# Installs ccn, the Claude Code Notifier CLI, on Windows.
#
#   irm https://claudecodenotifier.com/install.ps1 | iex; ccn pair 482913
#
# $env:CCN_VERSION = "0.2.0" picks a release (default: the latest).
# $env:CCN_INSTALL_DIR overrides %LOCALAPPDATA%\Programs\ccn.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'touch-tap/ccn'
$dir = if ($env:CCN_INSTALL_DIR) { $env:CCN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\ccn' }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$asset = "ccn-$arch-pc-windows-msvc.zip"
$base = if ($env:CCN_DOWNLOAD_BASE) { $env:CCN_DOWNLOAD_BASE } elseif ($env:CCN_VERSION) { "https://github.com/$repo/releases/download/v$($env:CCN_VERSION.TrimStart('v'))" } else { "https://github.com/$repo/releases/latest/download" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ccn-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading ccn for $arch Windows..."
    Invoke-WebRequest "$base/$asset" -OutFile (Join-Path $tmp $asset) -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing

    $expected = (Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | ForEach-Object { ($_ -split '\s+')[0] }) | Select-Object -First 1
    $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected.ToLower() -ne $actual) { throw "checksum mismatch for $asset" }

    Expand-Archive (Join-Path $tmp $asset) -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    # A running ccn.exe cannot be overwritten but can be renamed, so move it aside first
    $target = Join-Path $dir 'ccn.exe'
    if (Test-Path $target) {
        $old = Join-Path $dir 'ccn.old.exe'
        Remove-Item $old -Force -ErrorAction SilentlyContinue
        Move-Item $target $old -Force
    }
    Move-Item (Join-Path $tmp 'ccn.exe') $target
    Write-Host "Installed $(& $target --version) to $target"
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# On PATH for this session (so `ccn pair` works on the same line) and for new terminals
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" } else { '' }) + $dir), 'User')
}
if (-not (($env:Path -split ';') -contains $dir)) { $env:Path = "$env:Path;$dir" }

Write-Host ""
Write-Host "Next: ccn pair <the code on your phone>"
