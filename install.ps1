<#
.SYNOPSIS
  Compass one-line installer for Windows.

.DESCRIPTION
  irm https://raw.githubusercontent.com/QuirijnPolinoco/compass/main/install.ps1 | iex

  Downloads the prebuilt x86_64-pc-windows-msvc release binary, verifies its
  SHA-256 checksum, smoke-tests it, installs compass.exe, and adds it to your
  user PATH. Nothing is compiled. (Windows 11 on ARM64 runs the x64 build via
  emulation; Windows 10 on ARM64 cannot run x64 binaries.)

  Environment knobs:
    $env:COMPASS_VERSION      pin a release tag, e.g. v0.6.0   (default: latest)
    $env:COMPASS_INSTALL_DIR  where to install                 (default: $env:LOCALAPPDATA\Compass\bin)
    $env:COMPASS_UNINSTALL=1  remove compass.exe and take it back off your PATH
#>

$ErrorActionPreference = 'Stop'

# Resolve the install directory the same way for install and uninstall.
function Get-CompassInstallDir {
    if ($env:COMPASS_INSTALL_DIR) { return $env:COMPASS_INSTALL_DIR }
    return (Join-Path $env:LOCALAPPDATA 'Compass\bin')
}

# Read the user PATH *unexpanded* (DoNotExpandEnvironmentNames) so existing
# %VAR%-style entries are not frozen to their current expansion when we write
# the value back. [Environment]::GetEnvironmentVariable returns the EXPANDED
# value, which is the footgun this avoids.
function Get-UserPathRaw {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
    if ($null -eq $key) { return '' }
    try {
        $val = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($null -eq $val) { return '' }
        return [string]$val
    }
    finally { $key.Dispose() }
}

# Broadcast WM_SETTINGCHANGE so already-running Explorer (and the terminals it
# launches next) pick up the new PATH, matching what SetEnvironmentVariable does
# for us. Best-effort: a failure here just means "open a new session after logon".
function Send-EnvChange {
    try {
        if (-not ('Win32.NativeMethods' -as [type])) {
            Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        [void][Win32.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
    }
    catch {
        Write-Verbose "could not broadcast environment change ($_)"
    }
}

# Write the user PATH back, preserving REG_EXPAND_SZ when it still contains a
# %VAR%, then broadcast. Falls back to SetEnvironmentVariable (which freezes
# %VAR% entries but always works) if direct registry access is denied.
function Save-UserPathRaw {
    param([string]$Value)
    try {
        $kind = if ($Value -match '%[^%]+%') {
            [Microsoft.Win32.RegistryValueKind]::ExpandString
        }
        else {
            [Microsoft.Win32.RegistryValueKind]::String
        }
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        if ($null -eq $key) { $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment') }
        try { $key.SetValue('Path', $Value, $kind) } finally { $key.Dispose() }
        Send-EnvChange
    }
    catch {
        Write-Verbose "registry PATH write failed, falling back to SetEnvironmentVariable ($_)"
        [Environment]::SetEnvironmentVariable('Path', $Value, 'User')
    }
}

function Add-CompassToUserPath {
    param([string]$Dir)

    if ([string]::IsNullOrWhiteSpace($Dir)) { return }
    $norm = $Dir.TrimEnd('\')

    $userPath = Get-UserPathRaw

    $present = $userPath -split ';' |
        Where-Object { $_ -ne '' } |
        Where-Object { $_.TrimEnd('\') -ieq $norm }

    if ($present) {
        Write-Host "compass-install: $Dir already on your user PATH"
    }
    else {
        $newPath = $userPath.TrimEnd(';')
        if ($newPath -ne '') { $newPath += ';' }
        $newPath += $Dir
        Save-UserPathRaw -Value $newPath
        Write-Host "compass-install: added $Dir to your user PATH"
    }

    # Make `compass` resolvable in THIS session too (best effort).
    $inSession = $env:PATH -split ';' |
        Where-Object { $_ -ne '' } |
        Where-Object { $_.TrimEnd('\') -ieq $norm }
    if (-not $inSession) {
        $env:PATH = ($env:PATH.TrimEnd(';') + ';' + $Dir)
    }
}

function Remove-CompassFromUserPath {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([string]$Dir)

    if ([string]::IsNullOrWhiteSpace($Dir)) { return }
    $norm = $Dir.TrimEnd('\')

    $userPath = Get-UserPathRaw
    $kept = $userPath -split ';' |
        Where-Object { $_ -ne '' } |
        Where-Object { $_.TrimEnd('\') -ine $norm }

    $newPath = ($kept -join ';')
    if ($newPath -eq $userPath.Trim(';')) {
        Write-Host "compass-install: $Dir was not on your user PATH"
    }
    elseif ($PSCmdlet.ShouldProcess('user PATH', "remove $Dir")) {
        Save-UserPathRaw -Value $newPath
        Write-Host "compass-install: removed $Dir from your user PATH"
    }
}

# Fetch a URL to a file over HTTPS only, with a friendly message on 404 / other
# failures instead of a raw .NET exception.
function Get-CompassDownload {
    param([string]$Url, [string]$OutFile)

    if ($Url -notlike 'https://*') { throw "refusing to download non-HTTPS URL: $Url" }
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    }
    catch {
        $code = $null
        try { $code = [int]$_.Exception.Response.StatusCode } catch { $code = $null }
        if ($code -eq 404) {
            throw ("download failed: not found (404)`n  $Url`n" +
                   "  Is that a published release? Release tags are v-prefixed, e.g. COMPASS_VERSION=v0.6.0.")
        }
        throw "download failed for $Url`n  $($_.Exception.Message)"
    }
}

# Copy the new binary into place, renaming a locked (running/AV-held) compass.exe
# out of the way first so an UPGRADE while compass is in use still succeeds.
function Install-CompassBinary {
    param([string]$Source, [string]$Dest)
    try {
        Copy-Item -Path $Source -Destination $Dest -Force
    }
    catch {
        $old = "$Dest.old-$([Guid]::NewGuid().ToString('N'))"
        try { Move-Item -Path $Dest -Destination $old -Force }
        catch { throw "could not replace $Dest (is compass running?). Close it and re-run.`n  $($_.Exception.Message)" }
        Copy-Item -Path $Source -Destination $Dest -Force
        # The OS frees the old file once the running process exits; cleanup is best-effort.
        try { Remove-Item -Path $old -Force -ErrorAction Stop } catch { Write-Verbose "left $old in place (still locked)" }
    }
}

function Uninstall-Compass {
    $installDir = Get-CompassInstallDir
    $dest = Join-Path $installDir 'compass.exe'

    Write-Host "compass-install: uninstalling compass from $installDir"
    if (Test-Path $dest) {
        Remove-Item -Path $dest -Force
        Write-Host "compass-install: removed $dest"
    }
    else {
        Write-Host "compass-install: no compass.exe at $dest"
    }

    Remove-CompassFromUserPath -Dir $installDir

    # Remove our own dir if we created it and it's now empty (leave custom dirs alone).
    if ((Test-Path $installDir) -and -not $env:COMPASS_INSTALL_DIR) {
        if (-not (Get-ChildItem -Force -Path $installDir)) {
            Remove-Item -Path $installDir -Force
        }
    }

    Write-Host ""
    Write-Host "compass-install: done. Open a NEW terminal for the PATH change to apply."
}

function Install-Compass {
    $repo   = 'QuirijnPolinoco/compass'
    $target = 'x86_64-pc-windows-msvc'
    $asset  = "compass-$target.zip"

    # Force TLS 1.2 (Windows PowerShell 5.1 may not negotiate it by default).
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    catch {
        Write-Verbose "could not raise TLS to 1.2; using the default ($_)"
    }

    # Send default credentials to an authenticating corporate proxy (HTTP 407)
    # so Invoke-WebRequest doesn't fail behind one. Best-effort.
    try {
        $proxy = [System.Net.WebRequest]::DefaultWebProxy
        if ($proxy) { $proxy.Credentials = [System.Net.CredentialCache]::DefaultCredentials }
    }
    catch {
        Write-Verbose "could not set default proxy credentials ($_)"
    }

    $version = if ($env:COMPASS_VERSION) { $env:COMPASS_VERSION } else { 'latest' }
    # Release tags are v-prefixed; accept a bare COMPASS_VERSION=0.6.0 and fix it
    # up so it doesn't 404 against /releases/download/0.6.0/...
    if ($version -ne 'latest' -and $version -match '^[0-9]') {
        Write-Host "compass-install: note: release tags are v-prefixed; using v$version"
        $version = "v$version"
    }

    if ($version -eq 'latest') {
        $base = "https://github.com/$repo/releases/latest/download"
    }
    else {
        $base = "https://github.com/$repo/releases/download/$version"
    }

    $installDir = Get-CompassInstallDir

    Write-Host "compass-install: installing compass ($version) for $target"

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("compass-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    $oldProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try {
        $zip = Join-Path $tmp $asset
        $sha = "$zip.sha256"

        Write-Host "compass-install: downloading $asset"
        Get-CompassDownload -Url "$base/$asset"        -OutFile $zip
        Get-CompassDownload -Url "$base/$asset.sha256" -OutFile $sha

        Write-Host "compass-install: verifying checksum"
        $expected = (((Get-Content -Raw $sha) -split '\s+') | Where-Object { $_ -ne '' } | Select-Object -First 1)
        if ([string]::IsNullOrWhiteSpace($expected)) {
            throw "checksum file for $asset was empty"
        }
        $expected = $expected.Trim().ToLower()
        $actual   = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) {
            throw ("checksum mismatch for $asset`n  expected: $expected`n  actual:   $actual`n" +
                   "  Refusing to install a binary that does not match its published checksum.")
        }

        Write-Host "compass-install: unpacking"
        $extract = Join-Path $tmp 'extract'
        Expand-Archive -Path $zip -DestinationPath $extract -Force
        $src = Join-Path $extract 'compass.exe'
        if (-not (Test-Path $src)) { throw "archive $asset did not contain compass.exe" }

        if (-not (Test-Path $installDir)) {
            New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        }
        $dest = Join-Path $installDir 'compass.exe'
        Install-CompassBinary -Source $src -Dest $dest

        Write-Host "compass-install: installed compass to $dest"

        # Smoke-test: a wrong-arch build (e.g. x64 on Windows 10 ARM64) installs
        # fine but cannot launch. Surface that now instead of at first run.
        $ran = $false
        try { & $dest --help *> $null; $ran = ($LASTEXITCODE -eq 0) } catch { $ran = $false }
        if (-not $ran) {
            Write-Warning ("compass.exe was installed to $dest but did not run here. " +
                "On Windows 10 ARM64 the x64 build cannot run (only Windows 11 ARM64 emulates x64). " +
                "Otherwise build from source: cargo install --git https://github.com/QuirijnPolinoco/compass compass-cli")
        }

        Add-CompassToUserPath -Dir $installDir
    }
    finally {
        $ProgressPreference = $oldProgress
        Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "compass-install: done. Open a NEW terminal, then run: compass --help"
}

if ($env:COMPASS_UNINSTALL) {
    Uninstall-Compass
}
else {
    Install-Compass
}
