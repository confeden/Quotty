# Runs cargo, then deletes from target\ what earlier builds left behind and the
# current build no longer uses. Everything the current build uses stays, so the
# next build is exactly as incremental as it would have been.
#
#   tools\cargo.ps1 build --release
#   tools\cargo.ps1 test
#   tools\cargo.ps1 clippy
#   tools\cargo.ps1 -SweepOnly build --release   sweep without building first
#   tools\cargo.ps1 -DryRun -SweepOnly test      list what would go, delete nothing
#
# After cargo succeeds the same command is repeated as a no-op build with
# --message-format=json and CARGO_LOG, which names the fingerprint directory of
# every unit the build uses. Any other unit in that profile directory goes when
# a unit of the same kind has replaced it (an older dependency, an older Quotty,
# an older toolchain). A unit of a kind this command does not build (clippy,
# test --release, a dev build) stays until cargo has not looked at it for
# $GraceDays days. Incremental caches carry no unit hash: each one belongs to
# the unit whose compile window holds its newest session.

$GraceDays = 14
$root = Split-Path -Parent $PSScriptRoot

# ------------------------------------------------------------- arguments ----

function Get-Query([string[]]$Tokens) {
    $pre = [System.Collections.Generic.List[string]]::new()
    $i = 0
    while ($i -lt $Tokens.Count -and $Tokens[$i].StartsWith('+')) { $pre.Add($Tokens[$i]); $i++ }
    if ($i -ge $Tokens.Count) { return $null }
    $name = $Tokens[$i]
    if ($name -in 'build', 'b', 'run', 'r') { $sub = 'build' }
    elseif ($name -in 'check', 'c') { $sub = 'check' }
    elseif ($name -eq 'clippy') { $sub = 'clippy' }
    elseif ($name -in 'test', 't') { $sub = 'test' }
    elseif ($name -eq 'bench') { $sub = 'bench' }
    else { return $null }

    $opts = [System.Collections.Generic.List[string]]::new()
    $tail = [System.Collections.Generic.List[string]]::new()
    $inTail = $false; $skip = $false
    for ($j = $i + 1; $j -lt $Tokens.Count; $j++) {
        $t = $Tokens[$j]
        if ($inTail) { $tail.Add($t); continue }
        if ($skip) { $skip = $false; continue }
        if ($t -eq '--') { $inTail = $true; continue }
        if ($t -eq '--message-format') { $skip = $true; continue }
        if ($t -like '--message-format=*' -or $t -like '--timings*') { continue }
        # A fix run rebuilds on purpose; repeating it is not a no-op.
        if ($t -eq '--fix') { return $null }
        $opts.Add($t)
    }

    $q = [System.Collections.Generic.List[string]]::new()
    $q.AddRange($pre); $q.Add($sub); $q.AddRange($opts)
    $q.Add('--message-format=json')
    if ($sub -in 'test', 'bench' -and -not $opts.Contains('--no-run')) { $q.Add('--no-run') }
    if ($tail.Count -and $name -notin 'run', 'r') { $q.Add('--'); $q.AddRange($tail) }

    # Where the build most likely lands; only used to skip the sweep when
    # nothing new has appeared there since the last one.
    $prof = if ($sub -eq 'bench') { 'release' } else { 'dev' }
    $triple = $null
    for ($j = 0; $j -lt $opts.Count; $j++) {
        $t = $opts[$j]
        if ($t -in '--release', '-r') { $prof = 'release' }
        elseif ($t -eq '--profile' -and $j + 1 -lt $opts.Count) { $prof = $opts[$j + 1] }
        elseif ($t -like '--profile=*') { $prof = $t.Substring(10) }
        elseif ($t -eq '--target' -and $j + 1 -lt $opts.Count) { $triple = $opts[$j + 1] }
        elseif ($t -like '--target=*') { $triple = $t.Substring(9) }
    }
    $dir = if ($prof -in 'dev', 'test') { 'debug' } elseif ($prof -eq 'bench') { 'release' } else { $prof }
    $dirs = @(Join-Path $root "target\$dir")
    if ($triple) { $dirs += Join-Path $root "target\$triple\$dir" }

    @{ Args = $q.ToArray(); Dirs = $dirs }
}

function ConvertTo-ArgString([string]$s) {
    if ($s.Length -and $s -notmatch '[\s"]') { return $s }
    $sb = [System.Text.StringBuilder]::new('"')
    $slashes = 0
    foreach ($c in $s.ToCharArray()) {
        if ($c -eq [char]'\') { $slashes++; continue }
        if ($c -eq [char]'"') { [void]$sb.Append([char]'\', 2 * $slashes + 1); $slashes = 0 }
        elseif ($slashes) { [void]$sb.Append([char]'\', $slashes); $slashes = 0 }
        [void]$sb.Append($c)
    }
    [void]$sb.Append([char]'\', 2 * $slashes).Append('"')
    $sb.ToString()
}

# --------------------------------------------------------------- helpers ----

function Get-FingerprintNames([string]$ProfileDir) {
    $fp = "$ProfileDir\.fingerprint"
    if (-not [IO.Directory]::Exists($fp)) { return , [string[]]@() }
    $names = [IO.Directory]::GetDirectories($fp)
    for ($i = 0; $i -lt $names.Length; $i++) { $names[$i] = $names[$i].Substring($fp.Length + 1) }
    [Array]::Sort($names, [StringComparer]::Ordinal)
    , $names
}

# A profile directory needs no sweep while it holds exactly the units it had
# after the last one: a unit only turns stale when a new one replaces it. The
# sweep still runs weekly, for units of other kinds that age out.
function Test-Unchanged([string]$ProfileDir) {
    $state = "$ProfileDir\.sweep-state"
    if (-not [IO.File]::Exists($state)) { return $false }
    $lines = [IO.File]::ReadAllLines($state)
    $when = [DateTime]::MinValue
    if (-not $lines.Count -or -not [DateTime]::TryParse($lines[0], [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind, [ref]$when)) { return $false }
    if (([DateTime]::UtcNow - $when).TotalDays -ge 7) { return $false }
    $old = if ($lines.Count -gt 1) { $lines[1..($lines.Count - 1)] -join "`n" } else { '' }
    ((Get-FingerprintNames $ProfileDir) -join "`n") -ceq $old
}

function ConvertFrom-SessionTime([string]$s) {
    $v = [decimal]0
    foreach ($c in $s.ToCharArray()) {
        $d = '0123456789abcdefghijklmnopqrstuvwxyz'.IndexOf($c)
        if ($d -lt 0) { return $null }
        $v = $v * 36 + $d
    }
    # rustc names a session after the microseconds since the Unix epoch.
    try { [DateTime]::new(1970, 1, 1, 0, 0, 0, [DateTimeKind]::Utc).AddTicks([long]($v * 10)) } catch { $null }
}

function Get-HashIndex([string]$Dir, [bool]$Dirs) {
    $idx = @{}
    if (-not [IO.Directory]::Exists($Dir)) { return $idx }
    $entries = if ($Dirs) { [IO.Directory]::GetDirectories($Dir) } else { [IO.Directory]::GetFiles($Dir) }
    $pattern = if ($Dirs) { '-([0-9a-f]{16})$' } else { '-([0-9a-f]{16})\.' }
    foreach ($e in $entries) {
        $m = [regex]::Match([IO.Path]::GetFileName($e), $pattern)
        if (-not $m.Success) { continue }
        $h = $m.Groups[1].Value
        if (-not $idx.ContainsKey($h)) { $idx[$h] = [System.Collections.Generic.List[string]]::new() }
        $idx[$h].Add($e)
    }
    $idx
}

function Remove-Entry([string]$Path, [hashtable]$Tally) {
    $size = [long]0
    try {
        if ([IO.Directory]::Exists($Path)) {
            foreach ($f in ([IO.DirectoryInfo]$Path).GetFiles('*', 'AllDirectories')) { $size += $f.Length }
            if (-not $Tally.DryRun) { [IO.Directory]::Delete($Path, $true) }
        } elseif ([IO.File]::Exists($Path)) {
            $size = ([IO.FileInfo]$Path).Length
            if (-not $Tally.DryRun) { [IO.File]::Delete($Path) }
        } else { return $true }
    } catch {
        # Most likely a running exe; the next sweep tries again.
        $Tally.Failed++
        return $false
    }
    $Tally.Bytes += $size
    if ($Tally.DryRun) { $Tally.List.Add($Path.Substring($Tally.Base.Length + 1)) }
    $true
}

# ----------------------------------------------------------------- sweep ----

function Invoke-ProfileSweep([string]$ProfileDir, $Used, [bool]$Strict, [bool]$DryRun) {
    $rel = if ($ProfileDir.StartsWith("$root\")) { $ProfileDir.Substring($root.Length + 1) } else { $ProfileDir }

    # Hold cargo's own lock on the directory: nothing may build in it meanwhile.
    $lock = [IO.File]::Open((Join-Path $ProfileDir '.cargo-lock'), 'OpenOrCreate', 'ReadWrite', 'ReadWrite')
    $locked = $false
    for ($i = 0; $i -lt 50 -and -not $locked; $i++) {
        try { $lock.Lock(0, [long]::MaxValue); $locked = $true } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $locked) { $lock.Dispose(); Write-Host "sweep ${rel}: another cargo holds it, skipped"; return }

    try {
        $now = [DateTime]::UtcNow
        $grace = [TimeSpan]::FromDays($GraceDays)

        # Every unit: its roles (what kind of unit it is), fingerprint value,
        # the values of what it was built against, and when cargo last read it.
        $units = [System.Collections.Generic.List[hashtable]]::new()
        $byHash = @{}
        foreach ($dir in [IO.Directory]::GetDirectories((Join-Path $ProfileDir '.fingerprint'))) {
            $name = [IO.Path]::GetFileName($dir)
            $m = [regex]::Match($name, '^(.+)-([0-9a-f]{16})$')
            if (-not $m.Success) { continue }
            $u = @{
                Name = $name; Pkg = $m.Groups[1].Value; Hash = $m.Groups[2].Value; Dir = $dir
                Roles = [System.Collections.Generic.List[string]]::new()
                Crates = [System.Collections.Generic.List[string]]::new()
                Bins = [System.Collections.Generic.List[string]]::new()
                FpValues = [System.Collections.Generic.List[string]]::new()
                DepValues = [System.Collections.Generic.List[string]]::new()
                Seen = [DateTime]::MinValue; Start = $null; End = [DateTime]::MinValue
                Kept = $false; Why = ''
            }
            foreach ($fi in ([IO.DirectoryInfo]$dir).GetFiles()) {
                if ($fi.LastWriteTimeUtc -gt $u.End) { $u.End = $fi.LastWriteTimeUtc }
                if ($fi.Name -eq 'invoked.timestamp') { $u.Start = $fi.LastWriteTimeUtc; continue }
                if ($fi.Extension -ne '.json') { continue }
                $base = $fi.Name.Substring(0, $fi.Name.Length - 5)
                $json = [IO.File]::ReadAllText($fi.FullName)
                $profileHash = [regex]::Match($json, '"profile":(\d+)')
                $kind = [regex]::Match($json, '"compile_kind":(\d+)')
                $deps = [regex]::Matches($json, '\[\d+,"[^"]*",(?:true|false),(\d+)\]')
                if (-not $profileHash.Success -or -not $kind.Success -or ($json.Contains('"deps":[[') -and -not $deps.Count)) {
                    throw "unfamiliar fingerprint format in $name\$($fi.Name)"
                }
                $u.Roles.Add("$($u.Pkg)|$base|$($profileHash.Groups[1].Value)|$($kind.Groups[1].Value)")
                $u.Crates.Add('_' + ($base -replace '-', '_'))
                if ($base -match '^bin-(.+)$') { $u.Bins.Add($Matches[1]) }
                foreach ($d in $deps) { $u.DepValues.Add($d.Groups[1].Value) }

                # Cargo reads this file on every freshness check, so its access
                # time says when a build last used the unit. Reading it here
                # must not refresh that.
                $hashFile = "$dir\$base"
                if (-not [IO.File]::Exists($hashFile)) { continue }
                $seen = [IO.File]::GetLastAccessTimeUtc($hashFile)
                if ($seen -gt $u.Seen) { $u.Seen = $seen }
                $hex = [IO.File]::ReadAllText($hashFile).Trim()
                if ([IO.File]::GetLastAccessTimeUtc($hashFile) -ne $seen) {
                    try { [IO.File]::SetLastAccessTimeUtc($hashFile, $seen) } catch { }
                }
                if ($hex -match '^[0-9a-f]{16}$') {
                    # The file holds the value's bytes little-endian first.
                    $bytes = [byte[]]::new(8)
                    for ($i = 0; $i -lt 8; $i++) { $bytes[$i] = [Convert]::ToByte($hex.Substring(2 * $i, 2), 16) }
                    $u.FpValues.Add([BitConverter]::ToUInt64($bytes, 0).ToString())
                }
            }
            if ($u.Seen -eq [DateTime]::MinValue) { $u.Seen = $u.End }
            $units.Add($u)
            $byHash[$u.Hash] = $u
        }

        $current = [System.Collections.Generic.HashSet[string]]::new()
        foreach ($u in $units) {
            if (-not $Used.Contains($u.Name)) { continue }
            $u.Kept = $true; $u.Why = 'used'
            foreach ($r in $u.Roles) { [void]$current.Add($r) }
        }

        # After a clean no-op build every unit it uses was built against the
        # others it uses; if that does not hold, this script misreads cargo.
        if ($Strict) {
            $values = [System.Collections.Generic.HashSet[string]]::new()
            foreach ($u in $units) { if ($u.Kept) { foreach ($v in $u.FpValues) { [void]$values.Add($v) } } }
            foreach ($u in $units) {
                if (-not $u.Kept) { continue }
                foreach ($v in $u.DepValues) {
                    if (-not $values.Contains($v)) { throw "$($u.Name) depends on a unit the build does not name" }
                }
            }
        }

        # A unit of the same kind as one this build uses has been replaced. Of
        # the kinds it does not build, the newest unit of each stays while it is
        # in use; older ones of that kind were replaced as well.
        $latest = @{}
        foreach ($u in $units) {
            if ($u.Kept) { continue }
            $replaced = $false
            foreach ($r in $u.Roles) { if ($current.Contains($r)) { $replaced = $true; break } }
            if ($replaced) { $u.Why = 'replaced'; continue }
            $role = if ($u.Roles.Count) { $u.Roles -join ';' } else { $u.Name }
            $best = $latest[$role]
            if (-not $best -or $u.Seen -gt $best.Seen -or ($u.Seen -eq $best.Seen -and $u.End -gt $best.End)) {
                if ($best) { $best.Why = 'replaced' }
                $latest[$role] = $u
            } else {
                $u.Why = 'replaced'
            }
        }
        foreach ($u in $latest.Values) {
            if ($now - $u.Seen -lt $grace) { $u.Kept = $true; $u.Why = 'recent' } else { $u.Why = 'unused' }
        }

        # Whatever a kept unit was built against stays too: a build script run
        # of another kind looks just like this build's own.
        $byValue = @{}
        foreach ($u in $units) {
            foreach ($v in $u.FpValues) {
                if (-not $byValue.ContainsKey($v)) { $byValue[$v] = [System.Collections.Generic.List[hashtable]]::new() }
                $byValue[$v].Add($u)
            }
        }
        $queue = [System.Collections.Generic.Queue[hashtable]]::new()
        foreach ($u in $units) { if ($u.Kept) { $queue.Enqueue($u) } }
        while ($queue.Count) {
            $u = $queue.Dequeue()
            foreach ($v in $u.DepValues) {
                if (-not $byValue.ContainsKey($v)) { continue }
                foreach ($d in $byValue[$v]) {
                    if (-not $d.Kept) { $d.Kept = $true; $d.Why = 'needed'; $queue.Enqueue($d) }
                }
            }
        }

        $depsIdx = Get-HashIndex (Join-Path $ProfileDir 'deps') $false
        $examplesIdx = Get-HashIndex (Join-Path $ProfileDir 'examples') $false
        $buildIdx = Get-HashIndex (Join-Path $ProfileDir 'build') $true

        # A binary built for MSVC has no hash in its name: every build of it
        # overwrites deps\<bin>.exe, so those files go only with the last unit.
        $binKept = [System.Collections.Generic.HashSet[string]]::new()
        $binDead = [System.Collections.Generic.HashSet[string]]::new()
        foreach ($u in $units) {
            if ($depsIdx.ContainsKey($u.Hash)) { continue }
            foreach ($b in $u.Bins) { if ($u.Kept) { [void]$binKept.Add($b) } else { [void]$binDead.Add($b) } }
        }

        # Incremental caches, tied to units by time.
        $incDead = [System.Collections.Generic.List[string]]::new()
        $incRoot = Join-Path $ProfileDir 'incremental'
        $incSeen = 0; $incTied = 0
        if ([IO.Directory]::Exists($incRoot)) {
            foreach ($dir in [IO.Directory]::GetDirectories($incRoot)) {
                $m = [regex]::Match([IO.Path]::GetFileName($dir), '^(.+)-[0-9a-z]+$')
                if (-not $m.Success) { continue }
                $crate = '_' + $m.Groups[1].Value
                $newest = $null
                foreach ($s in [IO.Directory]::GetDirectories($dir)) {
                    $sm = [regex]::Match([IO.Path]::GetFileName($s), '^s-([0-9a-z]+)-')
                    if (-not $sm.Success) { continue }
                    $t = ConvertFrom-SessionTime $sm.Groups[1].Value
                    if ($t -and (-not $newest -or $t -gt $newest)) { $newest = $t }
                }
                if (-not $newest) { continue }
                $incSeen++
                $owners = 0; $keep = $false
                foreach ($u in $units) {
                    if (-not $u.Start) { continue }
                    if ($newest -lt $u.Start.AddMilliseconds(-200) -or $newest -gt $u.End.AddMilliseconds(200)) { continue }
                    $named = $false
                    foreach ($c in $u.Crates) { if ($c.EndsWith($crate)) { $named = $true; break } }
                    if (-not $named) { continue }
                    $owners++
                    if ($u.Kept) { $keep = $true }
                }
                if ($owners) { $incTied++ } else { $keep = $now - $newest -lt $grace }
                if (-not $keep) { $incDead.Add($dir) }
            }
            if ($incSeen -and -not $incTied) {
                Write-Warning "sweep ${rel}: no incremental cache matches a unit, leaving them all"
                $incDead.Clear()
            }
        }

        $tally = @{ Bytes = [long]0; Failed = 0; DryRun = $DryRun; Base = $ProfileDir
                    List = [System.Collections.Generic.List[string]]::new() }
        $gone = 0
        foreach ($u in $units) {
            if ($u.Kept) { continue }
            $ok = $true
            foreach ($idx in $depsIdx, $examplesIdx, $buildIdx) {
                if (-not $idx.ContainsKey($u.Hash)) { continue }
                foreach ($p in $idx[$u.Hash]) { if (-not (Remove-Entry $p $tally)) { $ok = $false } }
            }
            # The fingerprint goes last, so a half-removed unit is retried
            # rather than forgotten.
            if ($ok -and (Remove-Entry $u.Dir $tally)) { $gone++ }
        }
        foreach ($b in $binDead) {
            if ($binKept.Contains($b)) { continue }
            foreach ($p in "deps\$b.exe", "deps\$b.pdb", "deps\$b.d", "$b.exe", "$b.pdb", "$b.d") {
                [void](Remove-Entry (Join-Path $ProfileDir $p) $tally)
            }
        }
        foreach ($d in $incDead) { [void](Remove-Entry $d $tally) }
        # Outputs whose fingerprint is already gone can never be reused.
        foreach ($idx in $depsIdx, $examplesIdx, $buildIdx) {
            foreach ($h in @($idx.Keys)) {
                if ($byHash.ContainsKey($h)) { continue }
                foreach ($p in $idx[$h]) { [void](Remove-Entry $p $tally) }
            }
        }

        if ($DryRun) {
            foreach ($u in ($units | Sort-Object { $_.Why }, { $_.Name })) {
                if (-not $u.Kept -or $u.Why -ne 'used') { '  {0,-8} {1,-10} {2}' -f $(if ($u.Kept) { 'keep' } else { 'delete' }), $u.Why, $u.Name }
            }
            foreach ($p in $tally.List) { if ($p -notmatch '^\.fingerprint\\') { "  delete   $p" } }
        } else {
            $names = Get-FingerprintNames $ProfileDir
            [IO.File]::WriteAllText((Join-Path $ProfileDir '.sweep-state'),
                (@($now.ToString('o')) + $names) -join "`n")
        }

        $mb = $tally.Bytes / 1MB
        $size = if ($mb -ge 1024) { '{0:N2} GB' -f ($mb / 1024) } else { '{0:N1} MB' -f $mb }
        $verb = if ($DryRun) { 'would free' } else { 'freed' }
        $msg = "sweep ${rel}: nothing stale"
        if ($tally.Bytes) { $msg = "sweep ${rel}: $gone stale units, $($incDead.Count) incremental caches, $verb $size" }
        if ($tally.Failed) { $msg += " ($($tally.Failed) in use, left for next time)" }
        Write-Host $msg
    } finally {
        $lock.Dispose()
    }
}

function Invoke-Sweep([hashtable]$Query, [bool]$Tolerant, [bool]$DryRun) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $psi.Arguments = ($Query.Args | ForEach-Object { ConvertTo-ArgString $_ }) -join ' '
    $psi.WorkingDirectory = $root
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    $psi.EnvironmentVariables['CARGO_LOG'] = 'cargo::core::compiler::fingerprint=debug'
    $psi.EnvironmentVariables['CARGO_TERM_COLOR'] = 'never'
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    $p.WaitForExit()

    # Units by profile directory, as cargo names them.
    $used = @{}
    $logHashes = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($line in $err.Result.Split("`n")) {
        $k = $line.IndexOf('fingerprint at: ')
        if ($k -lt 0) { continue }
        $file = $line.Substring($k + 16).Trim()
        if (-not [IO.Path]::IsPathRooted($file)) { $file = Join-Path $root $file }
        $unitDir = [IO.Path]::GetDirectoryName($file)
        $fpRoot = [IO.Path]::GetDirectoryName($unitDir)
        if ([IO.Path]::GetFileName($fpRoot) -ne '.fingerprint') { continue }
        $prof = [IO.Path]::GetDirectoryName($fpRoot)
        if (-not $used.ContainsKey($prof)) {
            $used[$prof] = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        }
        $unit = [IO.Path]::GetFileName($unitDir)
        [void]$used[$prof].Add($unit)
        if ($unit -match '-([0-9a-f]{16})$') { [void]$logHashes.Add($Matches[1]) }
    }

    # Cross-check against the artifacts cargo reports: the hash in each one's
    # name must be a unit it named, or deleting by hash would hit live files.
    # (Paths inside the JSON have their backslashes doubled.)
    $json = $out.Result
    $finished = $json.Contains('{"reason":"build-finished"')
    foreach ($m in [regex]::Matches($json, '\\\\(?:deps|build|examples)\\\\[^"\\]*?-([0-9a-f]{16})(?=[.\\"])')) {
        if (-not $logHashes.Contains($m.Groups[1].Value)) {
            throw "cargo reports an artifact of unit $($m.Groups[1].Value), which it did not name"
        }
    }

    if (-not $used.Count) { throw "cargo named no units (exit $($p.ExitCode)); is CARGO_LOG still supported?" }
    if ($p.ExitCode -ne 0 -and -not ($Tolerant -and $finished)) { throw "the no-op build failed (exit $($p.ExitCode))" }

    foreach ($prof in $used.Keys) {
        Invoke-ProfileSweep $prof $used[$prof] ($p.ExitCode -eq 0) $DryRun
    }
}

# ------------------------------------------------------------------ main ----

$sweepOnly = $false; $dryRun = $false
$cargoArgs = [System.Collections.Generic.List[string]]::new()
foreach ($a in $args) {
    if (-not $cargoArgs.Count -and $a -eq '-SweepOnly') { $sweepOnly = $true; continue }
    if (-not $cargoArgs.Count -and $a -eq '-DryRun') { $dryRun = $true; continue }
    $cargoArgs.Add([string]$a)
}
$cargoArgs = $cargoArgs.ToArray()

$exit = 0
Push-Location $root
try {
    if (-not $sweepOnly) {
        & cargo @cargoArgs
        $exit = $LASTEXITCODE
    }
    $query = Get-Query $cargoArgs
    if ($exit -eq 0 -and $query) {
        # A failed sweep costs disk space, never the build.
        try {
            $fresh = @($query.Dirs | Where-Object { -not (Test-Unchanged $_) })
            if ($sweepOnly -or $dryRun -or $fresh.Count) { Invoke-Sweep $query $sweepOnly $dryRun }
        } catch {
            Write-Warning "target sweep skipped: $($_.Exception.Message)"
        }
    } elseif ($sweepOnly -and -not $query) {
        Write-Warning "nothing to sweep for: cargo $cargoArgs"
    }
} finally {
    Pop-Location
}
exit $exit
