<#
.SYNOPSIS
Builds every plugin to WebAssembly, ready to install or publish.

.DESCRIPTION
Each plugin in plugins/ is built for wasm32-unknown-unknown and written to dist/<id>/ as plugin.json and plugin.wasm,
the folder Task Proxy's "Install from folder" takes.

-Sign signs both files with the Task Proxy update key, as CI does before publishing, from TAURI_SIGNING_PRIVATE_KEY
and TAURI_SIGNING_PRIVATE_KEY_PASSWORD. -Catalog then writes dist/catalog.json for the CDN. Each entry lists the plugin
spec versions it supports, and, from the catalog already published, the older versions that support a spec line the
new one dropped, so a Task Proxy that is not updated still finds the newest version it can run (spec/README.md).

-UpdateAppFixtures copies the test plugin into the TaskProxyApp checkout beside this repo, for its Rust tests, signed
with a new key made only for the tests, whose private half is deleted.
#>
[CmdletBinding()]
param(
    [switch]$Sign,
    [switch]$Catalog,
    [switch]$UpdateAppFixtures
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$catalogRoot = 'https://cdn.myfi.ws/apps/task-proxy/plugins'

function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE) { throw "$What failed with exit code $LASTEXITCODE." }
}

# A spec line is major.minor. An entry from before spec versions names only apiVersion 1, which is line 0.1.
function Get-SpecLines($Entry) {
    $specs = @($Entry.spec | Where-Object { $_ })
    if (-not $specs.Count -and $Entry.apiVersion -eq 1) { $specs = @('0.1.0') }
    @($specs | ForEach-Object { ($_ -split '\.')[0..1] -join '.' } | Sort-Object -Unique)
}

function ConvertTo-VersionKey([string]$Version) {
    [version]((($Version -split '\.') + @('0', '0', '0'))[0..2] -join '.')
}

# The older versions to keep listed beside a new one: for each spec line the new version does not support, the
# newest older version that does.
function Get-KeptVersions($New, $Published) {
    $published = @($Published | Where-Object { $_.id -eq $New.id } | ForEach-Object {
            $_
            @($_.versions | Where-Object { $_ })
        } | Where-Object { (ConvertTo-VersionKey $_.version) -lt (ConvertTo-VersionKey $New.version) })
    $covered = Get-SpecLines $New
    $kept = [ordered]@{}
    foreach ($old in $published | Sort-Object { ConvertTo-VersionKey $_.version } -Descending) {
        foreach ($line in Get-SpecLines $old) {
            if ($covered -contains $line -or $kept.Contains($line)) { continue }
            $kept[$line] = $old
        }
    }
    $seen = @{}
    @($kept.Values | Where-Object { -not $seen.ContainsKey($_.version) -and ($seen[$_.version] = $true) } | ForEach-Object {
            $entry = [ordered]@{}
            $_.PSObject.Properties | Where-Object { $_.Name -ne 'versions' } | ForEach-Object { $entry[$_.Name] = $_.Value }
            $entry
        })
}

function Invoke-Signer([string]$File, [string]$KeyFile, [string]$Password) {
    $arguments = @('@tauri-apps/cli@2', 'signer', 'sign')
    if ($KeyFile) { $arguments += @('-f', $KeyFile, '-p', $Password) }
    $arguments += $File
    Invoke-Native "Signing $File" { npx --yes @arguments | Out-Null }
}

Push-Location $root
try {
    Invoke-Native 'Building the plugins' { cargo build --release --target wasm32-unknown-unknown }
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $built = Join-Path $targetDir 'wasm32-unknown-unknown/release'
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $entries = @()
    $published = @()
    if ($Catalog) {
        try {
            $published = @(Invoke-RestMethod "$catalogRoot/catalog.json")
        }
        catch {
            Write-Warning "The published catalog could not be read, so no older versions are kept: $($_.Exception.Message)"
        }
    }
    foreach ($folder in Get-ChildItem (Join-Path $root 'plugins') -Directory) {
        $manifest = Get-Content (Join-Path $folder.FullName 'plugin.json') -Raw | ConvertFrom-Json
        if (-not @($manifest.spec | Where-Object { $_ -match '^\d+\.\d+\.\d+$' }).Count) {
            throw "$($folder.Name)/plugin.json must list the plugin spec versions it supports, as `"spec`": [`"0.1.0`"] (spec/README.md)."
        }
        $crate = (Select-String -Path (Join-Path $folder.FullName 'Cargo.toml') -Pattern '^name\s*=\s*"(.+)"').Matches[0].Groups[1].Value
        $out = Join-Path $dist $manifest.id
        New-Item -ItemType Directory -Force -Path $out | Out-Null
        Copy-Item (Join-Path $folder.FullName 'plugin.json') (Join-Path $out 'plugin.json')
        Copy-Item (Join-Path $built "$($crate -replace '-', '_').wasm") (Join-Path $out 'plugin.wasm')
        Write-Host "Built $($manifest.name) $($manifest.version) into $out" -ForegroundColor Green
        if ($Sign) {
            Invoke-Signer (Join-Path $out 'plugin.json')
            Invoke-Signer (Join-Path $out 'plugin.wasm')
        }
        if ($Catalog) {
            $base = "$catalogRoot/$($manifest.id)/$($manifest.version)"
            $entry = [ordered]@{
                id = $manifest.id; name = $manifest.name; version = $manifest.version
                spec = @($manifest.spec); apiVersion = $manifest.apiVersion
                description = $manifest.description; category = $manifest.category; icon = $manifest.icon
                permissions = $manifest.permissions
                manifest = "$base/plugin.json"; manifestSignature = (Get-Content (Join-Path $out 'plugin.json.sig') -Raw).Trim()
                wasm = "$base/plugin.wasm"; wasmSignature = (Get-Content (Join-Path $out 'plugin.wasm.sig') -Raw).Trim()
            }
            $kept = @(Get-KeptVersions ([pscustomobject]$entry) $published)
            if ($kept.Count) { $entry.versions = $kept }
            $entries += $entry
        }
    }
    if ($Catalog) {
        ConvertTo-Json -InputObject @($entries) -Depth 10 | Set-Content (Join-Path $dist 'catalog.json')
        Write-Host "Wrote $(Join-Path $dist 'catalog.json')" -ForegroundColor Green
    }
    if ($UpdateAppFixtures) {
        $app = Join-Path (Split-Path $root -Parent) 'TaskProxyApp/src-tauri/tests/fixtures/plugins'
        if (-not (Test-Path $app)) { throw "The TaskProxyApp checkout was not found at $app." }
        $fixture = Join-Path $app 'test'
        Copy-Item (Join-Path $root 'test-plugin/plugin.json') (Join-Path $fixture 'plugin.json')
        Copy-Item (Join-Path $built 'taskproxy_test_plugin.wasm') (Join-Path $fixture 'plugin.wasm')
        $work = Join-Path ([System.IO.Path]::GetTempPath()) ("tp-key-" + [guid]::NewGuid())
        New-Item -ItemType Directory -Path $work | Out-Null
        try {
            $key = Join-Path $work 'test.key'
            Invoke-Native 'Making a test key' { npx --yes '@tauri-apps/cli@2' signer generate -w $key -p '' --ci -f | Out-Null }
            Invoke-Signer (Join-Path $fixture 'plugin.json') $key ''
            Invoke-Signer (Join-Path $fixture 'plugin.wasm') $key ''
            Copy-Item "$key.pub" (Join-Path $app 'test-signing.key.pub')
        }
        finally {
            Remove-Item $work -Recurse -Force
        }
        Write-Host "Updated the test plugin in $fixture" -ForegroundColor Green
    }
}
finally {
    Pop-Location
}
