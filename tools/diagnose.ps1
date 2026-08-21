<#
    Quotty diagnostics — collects everything needed to explain why a source is
    offline, without changing the program. Writes a single report next to
    itself and prints it.

        powershell -ExecutionPolicy Bypass -File tools\diagnose.ps1

    Nothing is uploaded anywhere. Tokens are never printed: only their length
    and a masked prefix, so the report is safe to send to the author.
#>
param([string]$Out)

# Default next to the script, but fall back to the temp folder: the script may
# sit in a read-only place, or be pasted into a console where $PSScriptRoot is
# empty.
if (-not $Out) {
    $dir = if ($PSScriptRoot) { $PSScriptRoot } else { $env:TEMP }
    $Out = Join-Path $dir 'quotty-report.txt'
}

$r = [System.Collections.Generic.List[string]]::new()
function Add($t) { $r.Add([string]$t) }
function Mask($s) {
    if (-not $s) { return '<empty>' }
    $s = [string]$s
    if ($s.Length -le 12) { return '<' + $s.Length + ' chars>' }
    return $s.Substring(0, 8) + '…<' + $s.Length + ' chars>'
}

Add "=== Quotty diagnostics ==="
Add ("time         : " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'))
Add ("windows      : " + (Get-CimInstance Win32_OperatingSystem).Caption + ' ' + [Environment]::OSVersion.Version)
Add ("elevated     : " + ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
Add ("APPDATA      : " + $env:APPDATA)
Add ("LOCALAPPDATA : " + $env:LOCALAPPDATA)

# ---------------------------------------------------------------- Quotty ----
Add ""
Add "--- Quotty ---"
$exes = @()
$proc = Get-Process -Name quotty -ErrorAction SilentlyContinue
foreach ($p in $proc) { $exes += $p.Path; Add ("running      : pid " + $p.Id + '  ' + $p.Path) }
if (-not $proc) { Add "running      : no" }
foreach ($c in @("$env:LOCALAPPDATA\Programs\Quotty\quotty.exe", "$env:ProgramFiles\Quotty\quotty.exe")) {
    if (Test-Path $c) { $exes += $c; Add ("installed    : " + $c) }
}
foreach ($exe in ($exes | Select-Object -Unique)) {
    Add ("version      : " + (Get-Item $exe).VersionInfo.FileVersion + '  (' + $exe + ')')
    $log = Join-Path (Split-Path $exe) 'quotty-debug.log'
    if (Test-Path $log) {
        Add ("debug log    : " + $log + '  ' + (Get-Item $log).Length + ' bytes, last write ' + (Get-Item $log).LastWriteTime)
        $lines = Get-Content $log -Encoding UTF8 -ErrorAction SilentlyContinue
        $codes = @{}
        foreach ($l in $lines) {
            if ($l -match '->\s*status\s*(\d{3})') { $codes[$Matches[1]] = 1 + [int]$codes[$Matches[1]] }
            elseif ($l -match '->\s*(network[^(]*)') { $codes['network'] = 1 + [int]$codes['network'] }
        }
        if ($codes.Count) {
            Add ("request results: " + (($codes.GetEnumerator() | Sort-Object Name | ForEach-Object { $_.Name + ' x' + $_.Value }) -join ', '))
        }
        Add "  --- last 25 lines (tokens masked) ---"
        $lines | Select-Object -Last 25 | ForEach-Object {
            Add ('  ' + ($_ -replace '(sk-[A-Za-z0-9_\-]{6})[A-Za-z0-9_\-]+', '$1...'))
        }
    } else {
        Add ("debug log    : none next to " + $exe + "  (that file only appears on failures)")
    }
}
$settings = Join-Path $env:APPDATA 'Quotty\settings.json'
Add ("settings     : " + $settings + '  exists=' + (Test-Path $settings))

# ---------------------------------------------------------------- Claude ----
Add ""
Add "--- Claude Desktop ---"
$claudeProc = Get-Process -Name claude -ErrorAction SilentlyContinue
Add ("running      : " + $(if ($claudeProc) { ($claudeProc | Measure-Object).Count.ToString() + ' process(es)' } else { 'no' }))
$pkg = Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'Claude|Anthropic' }
if ($pkg) { foreach ($p in $pkg) { Add ("store build  : " + $p.Name + ' ' + $p.Version + '  family=' + $p.PackageFamilyName) } }
else { Add "store build  : not installed from the Microsoft Store" }

# The same candidate list Quotty walks.
$dirs = @((Join-Path $env:APPDATA 'Claude'))
Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Packages') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^(?i)claude' -or $_.Name -match '(?i)anthropic' } |
    ForEach-Object {
        $dirs += (Join-Path $_.FullName 'LocalCache\Roaming\Claude')
        $dirs += (Join-Path $_.FullName 'LocalCache\Local\Claude')
    }

foreach ($d in ($dirs | Select-Object -Unique)) {
    $cfg = Join-Path $d 'config.json'
    $ls  = Join-Path $d 'Local State'
    if (-not (Test-Path $d)) { Add ("candidate    : " + $d + '  — no such directory'); continue }
    Add ("candidate    : " + $d)
    foreach ($f in @($cfg, $ls)) {
        if (Test-Path $f) {
            $i = Get-Item $f
            Add ("   " + $i.Name.PadRight(12) + ' ' + $i.Length.ToString().PadLeft(8) + ' bytes, last write ' + $i.LastWriteTime)
        } else {
            Add ("   " + (Split-Path $f -Leaf).PadRight(12) + ' MISSING')
        }
    }
    if (Test-Path $cfg) {
        try {
            $json = Get-Content $cfg -Raw -ErrorAction Stop | ConvertFrom-Json
            $keys = $json.PSObject.Properties.Name
            $oauth = $keys | Where-Object { $_ -match '(?i)oauth' }
            Add ("   keys         : " + ($keys -join ', '))
            foreach ($k in $oauth) {
                $v = $json.$k
                Add ("   " + $k + ' : ' + $(if ($v -is [string]) { 'encrypted blob, ' + (Mask $v) } else { $v.PSObject.Properties.Name -join ', ' }))
            }
        } catch { Add ("   config.json  : unreadable/unparseable — " + $_.Exception.Message) }
    }
    if (Test-Path $ls) {
        try {
            $lsj = Get-Content $ls -Raw -ErrorAction Stop | ConvertFrom-Json
            Add ("   os_crypt key : " + (Mask $lsj.os_crypt.encrypted_key))
        } catch { Add ("   Local State  : unreadable/unparseable — " + $_.Exception.Message) }
    }
}

# --------------------------------------------------------------- network ----
Add ""
Add "--- network to api.anthropic.com ---"
try {
    $ip = [System.Net.Dns]::GetHostAddresses('api.anthropic.com') | ForEach-Object { $_.IPAddressToString }
    Add ("dns          : " + ($ip -join ', '))
} catch { Add ("dns          : FAILED — " + $_.Exception.Message) }

try {
    $c = New-Object System.Net.Sockets.TcpClient
    $ok = $c.ConnectAsync('api.anthropic.com', 443).Wait(6000)
    Add ("tcp 443      : " + $(if ($ok -and $c.Connected) { 'connected' } else { 'FAILED (blocked or no route)' }))
    $c.Close()
} catch { Add ("tcp 443      : FAILED — " + $_.Exception.Message) }

# Without a token the endpoint must answer 401/403 — a *reply* proves the path
# works and points the blame at the token; a transport error means it does not.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $resp = Invoke-WebRequest -Uri 'https://api.anthropic.com/api/oauth/usage' -Method GET -TimeoutSec 15 -UseBasicParsing -ErrorAction Stop
    Add ("https probe  : HTTP " + [int]$resp.StatusCode + ' (unexpected without a token)')
} catch [System.Net.WebException] {
    $st = $_.Exception.Response
    if ($st) { Add ("https probe  : HTTP " + [int]$st.StatusCode + ' — the endpoint is reachable') }
    else { Add ("https probe  : no reply — " + $_.Exception.Message) }
} catch { Add ("https probe  : " + $_.Exception.Message) }

Add ""
Add "--- proxy (Claude honours the system proxy, Quotty talks direct) ---"
Add ("winhttp      : " + ((netsh winhttp show proxy) -join ' / '))
$ie = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction SilentlyContinue
Add ("ie proxy     : enable=" + $ie.ProxyEnable + ' server=' + $ie.ProxyServer)
Add ("env          : HTTPS_PROXY=" + $env:HTTPS_PROXY + ' HTTP_PROXY=' + $env:HTTP_PROXY)

Add ""
Add "--- how to read this ---"
Add "https probe 401/403 : the endpoint is reachable, so the problem is the token"
Add "                      (Claude Desktop signed out, or its token was rotated)"
Add "https probe 429     : Anthropic is rate-limiting this IP; every source stays"
Add "                      offline until it lets go — waiting is the only cure"
Add "https probe no reply: no route at all — region block, firewall, VPN off"
Add "config.json MISSING : Quotty is looking in the wrong place; send the candidate list"
Add "debug log absent    : Quotty has not failed since it started — restart it, reproduce,"
Add "                      then run this script again"

try {
    $r | Set-Content -Path $Out -Encoding UTF8 -ErrorAction Stop
} catch {
    $Out = Join-Path $env:TEMP 'quotty-report.txt'
    $r | Set-Content -Path $Out -Encoding UTF8
}
Write-Output ($r -join [Environment]::NewLine)
Write-Output ""
Write-Output ("Report saved to: " + $Out)
