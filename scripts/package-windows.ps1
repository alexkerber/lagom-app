# Build lagom.exe and its per-user installer, target\windows\lagom-<version>-windows-x64-setup.exe.
# Usage: scripts\package-windows.ps1 [version]   (PowerShell 7.3+, Rust with the MSVC toolchain, Inno Setup 6)
# The version is the release tag without the v (docs/release.md); without one it is 0.0.0-dev. Google and Microsoft sign-in need LAGOM_GOOGLE_CLIENT_ID,
# LAGOM_GOOGLE_CLIENT_SECRET and LAGOM_MICROSOFT_CLIENT_ID in the environment at build time.
# Signing is optional: set WINDOWS_CERTIFICATE (base64 .pfx) and WINDOWS_CERTIFICATE_PASSWORD, and
# optionally WINDOWS_TIMESTAMP_URL. Without them the installer is unsigned.
#Requires -Version 7.3
$ErrorActionPreference = 'Stop'
# A native command that exits non-zero stops the script too.
$PSNativeCommandUseErrorActionPreference = $true
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$version = if ($args.Count -gt 0) { $args[0] } else { '0.0.0-dev' }
if ($version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$') { throw "Version must look like 1.2.3 or 1.2.3-beta.1, got $version" }
# build.rs reads it for VERSIONINFO.
$env:LAGOM_VERSION = $version
# VERSIONINFO takes four numbers: 1.2.3-beta.1 becomes 1.2.3.0.
$numeric = ($version -split '[-+]')[0] + '.0'
# Static CRT: a clean Windows has no VCRUNTIME140.dll, and the installer ships no VC++ redistributable.
$env:RUSTFLAGS = "$env:RUSTFLAGS -C target-feature=+crt-static".Trim()
cargo build --release --locked -p lagom

$out = Join-Path $repo 'target\windows'
$cache = Join-Path $out 'cache'
New-Item -ItemType Directory -Force $cache | Out-Null
$bootstrapper = Join-Path $cache 'MicrosoftEdgeWebview2Setup.exe'
if (-not (Test-Path $bootstrapper)) {
    Invoke-WebRequest 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper
}
# The installer runs this file: accept only Microsoft's signed bootstrapper.
$signature = Get-AuthenticodeSignature $bootstrapper
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
    Remove-Item $bootstrapper
    throw "WebView2 bootstrapper signature is not Microsoft's: $($signature.Status)"
}

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { throw 'Inno Setup 6 not found; install it with: winget install JRSoftware.InnoSetup' }

$defines = @(
    "/DVersion=$version", "/DNumericVersion=$numeric", "/DRepo=$repo",
    "/DExe=$repo\target\release\lagom.exe", "/DBootstrapper=$bootstrapper", "/DOutputDir=$out"
)
$pfx = $null
try {
    if ($env:WINDOWS_CERTIFICATE) {
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
            Sort-Object { [version]$_.Directory.Parent.Name } | Select-Object -Last 1
        if (-not $signtool) { throw 'signtool.exe not found; install the Windows SDK' }
        $pfx = Join-Path ([IO.Path]::GetTempPath()) "lagom-$([guid]::NewGuid()).pfx"
        [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE))
        $timestamp = if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
        # Windows argument quoting (backslashes before a quote or the closing quote double), then
        # Inno Setup's: $q is a quote, $f the quoted file and a literal $ is $$.
        $password = ("$env:WINDOWS_CERTIFICATE_PASSWORD" -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1').Replace('$', '$$')
        $sign = '$q{0}$q sign /fd sha256 /td sha256 /tr {1} /f $q{2}$q /p $q{3}$q $f' -f $signtool.FullName, $timestamp, $pfx, $password
        $defines += '/DSign', "/Slagom=$sign"
    } else {
        Write-Warning 'No WINDOWS_CERTIFICATE: the installer is unsigned, so SmartScreen will warn.'
    }
    & $iscc /Q @defines (Join-Path $PSScriptRoot 'package-windows.iss')
} finally {
    if ($pfx) { Remove-Item -Force -ErrorAction SilentlyContinue $pfx }
}
Join-Path $out "lagom-$version-windows-x64-setup.exe"
