<#
.SYNOPSIS
Prepares the plugins for a commit: formats the code, gives each changed plugin a new version, then runs the checks CI
runs and builds every plugin.

.DESCRIPTION
Run this before every commit, or select it in Task Proxy's Git Controller to run it before each commit. If any step
fails, it stops and fails, so the commit is cancelled. A push to main publishes every plugin, so a plugin that changed
needs a new version, or installed copies would never update.

Steps:
1. Format the Rust code with cargo fmt.
2. Increment the version of each plugin that changed since the last commit, in its Cargo.toml and plugin.json, unless
   it already has a new version. A change to the SDK changes every plugin. Running this again before committing does
   not increment it twice.
3. Check that every plugin's Cargo.toml and plugin.json versions match.
4. Lint with clippy, failing on any warning, as CI does.
5. Run every test.
6. Build every plugin to WebAssembly, into dist/<id>.
7. Write the Jira demo screens to plugins/jira/demo, which TaskProxyApp's website capture reads from this checkout.

Nothing here writes into TaskProxyApp, so publishing plugins never needs an app commit or release. Its tests keep the
test plugin they were written for; when the test plugin itself changes, this says so, and build.ps1
-UpdateAppFixtures copies it there for a test that needs it.

.PARAMETER Version
Which part of a changed plugin's version to increment.

.PARAMETER SkipVersionBump
Leave versions as they are.
#>
[CmdletBinding()]
param(
    [ValidateSet('patch', 'minor', 'major')]
    [string]$Version = 'patch',
    [switch]$SkipVersionBump
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }

function Invoke-Check([string]$Name, [scriptblock]$Command) {
    Write-Host "== $Name" -ForegroundColor Cyan
    $global:LASTEXITCODE = 0
    & $Command
    if ($LASTEXITCODE) { throw "$Name failed with exit code $LASTEXITCODE." }
}

function Test-Committed {
    git rev-parse --verify --quiet HEAD *> $null
    return $LASTEXITCODE -eq 0
}

# Whether anything under these paths differs from the last commit, including new files.
function Test-Changed([string[]]$Paths) {
    if (-not (Test-Committed)) { return $true }
    $status = git status --porcelain --untracked-files=all -- @Paths
    return [bool]$status
}

function Get-CargoVersion([string]$Text) {
    $match = [regex]::Match($Text, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $match.Success) { throw 'No version found in Cargo.toml.' }
    return $match.Groups[1].Value
}

function Get-ManifestVersion([string]$Text) {
    $match = [regex]::Match($Text, '"version"\s*:\s*"([^"]+)"')
    if (-not $match.Success) { throw 'No version found in plugin.json.' }
    return $match.Groups[1].Value
}

function Step-Version([string]$Current, [string]$Part) {
    $parts = $Current.Split('.') | ForEach-Object { [int]$_ }
    switch ($Part) {
        'major' { return "$($parts[0] + 1).0.0" }
        'minor' { return "$($parts[0]).$($parts[1] + 1).0" }
        default { return "$($parts[0]).$($parts[1]).$($parts[2] + 1)" }
    }
}

# Writes text back without changing the file's line endings or adding a final newline it did not have.
function Set-Text([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text)
}

function Update-PluginVersions {
    Write-Host "== Increment the $Version version of changed plugins" -ForegroundColor Cyan
    $committed = Test-Committed
    $sdkChanged = Test-Changed @('sdk')
    foreach ($folder in Get-ChildItem (Join-Path $root 'plugins') -Directory) {
        $relative = "plugins/$($folder.Name)"
        # Its demo screens only follow its code, so they alone are no reason for a new version.
        if (-not $sdkChanged -and -not (Test-Changed @($relative, ":(exclude)$relative/demo"))) {
            Write-Host "  $($folder.Name): unchanged"
            continue
        }
        $cargoPath = Join-Path $folder.FullName 'Cargo.toml'
        $manifestPath = Join-Path $folder.FullName 'plugin.json'
        $cargo = Get-Content $cargoPath -Raw
        $manifest = Get-Content $manifestPath -Raw
        $current = Get-ManifestVersion $manifest
        $previous = $null
        if ($committed) {
            $old = git show "HEAD:$relative/plugin.json" 2>$null
            if ($LASTEXITCODE -eq 0) { $previous = Get-ManifestVersion ($old -join "`n") }
        }
        if (-not $previous) {
            Write-Host "  $($folder.Name): new, keeping $current"
            continue
        }
        if ($current -ne $previous) {
            Write-Host "  $($folder.Name): already $current (was $previous)"
            continue
        }
        $next = Step-Version $current $Version
        $cargoVersion = Get-CargoVersion $cargo
        Set-Text $manifestPath ([regex]::new('"version"(\s*):(\s*)"[^"]+"').Replace($manifest, "`"version`"`$1:`$2`"$next`"", 1))
        Set-Text $cargoPath ([regex]::new('(?m)^version(\s*)=(\s*)"' + [regex]::Escape($cargoVersion) + '"').Replace($cargo, "version`$1=`$2`"$next`"", 1))
        Write-Host "  $($folder.Name): $current -> $next" -ForegroundColor Green
    }
}

function Assert-VersionsMatch {
    Write-Host '== Check that each plugin has one version' -ForegroundColor Cyan
    foreach ($folder in Get-ChildItem (Join-Path $root 'plugins') -Directory) {
        $cargo = Get-CargoVersion (Get-Content (Join-Path $folder.FullName 'Cargo.toml') -Raw)
        $manifest = Get-ManifestVersion (Get-Content (Join-Path $folder.FullName 'plugin.json') -Raw)
        if ($cargo -ne $manifest) {
            throw "$($folder.Name) is version $cargo in Cargo.toml but $manifest in plugin.json."
        }
    }
}

Push-Location $root
try {
    Invoke-Check 'Format the Rust code' { cargo fmt --all }
    if (-not $SkipVersionBump) { Update-PluginVersions }
    Assert-VersionsMatch
    Invoke-Check 'Lint' { cargo clippy --workspace --all-targets -- -D warnings }
    Invoke-Check 'Run the tests' { cargo test --workspace }
    Invoke-Check 'Build the plugins' { & (Join-Path $PSScriptRoot 'build.ps1') }
    Invoke-Check 'Write the Jira demo screens' {
        cargo run --quiet -p taskproxy-jira --example demo -- (Join-Path $root 'plugins/jira/demo') | Out-Null
    }
    if (Test-Changed @('test-plugin')) {
        Write-Host 'The test plugin changed. If a TaskProxyApp test needs the change, run ./scripts/build.ps1 -UpdateAppFixtures and commit it there.' -ForegroundColor Yellow
    }
    Write-Host 'Ready to commit.' -ForegroundColor Green
}
finally {
    Pop-Location
}
