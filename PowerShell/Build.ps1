<#
.SYNOPSIS
Builds every plugin to WebAssembly, ready to install or publish.

.DESCRIPTION
Each plugin in plugins/ is built for wasm32-unknown-unknown and written to dist/<id>/ as plugin.json and plugin.wasm,
the folder Task Proxy's "Install from folder" takes.

-Sign signs both files with the Task Proxy update key, as CI does before publishing, from TAURI_SIGNING_PRIVATE_KEY
and TAURI_SIGNING_PRIVATE_KEY_PASSWORD. -Catalog then writes dist/catalog.json for the CDN.

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

function Invoke-Signer([string]$File, [string]$KeyFile, [string]$Password) {
    $arguments = @('@tauri-apps/cli@2', 'signer', 'sign')
    if ($KeyFile) { $arguments += @('-f', $KeyFile, '-p', $Password) }
    $arguments += $File
    Invoke-Native "Signing $File" { npx --yes @arguments | Out-Null }
}

Push-Location $root
try {
    Invoke-Native 'Building the plugins' { cargo build --release --target wasm32-unknown-unknown }
    $built = Join-Path $root 'target/wasm32-unknown-unknown/release'
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $entries = @()
    foreach ($folder in Get-ChildItem (Join-Path $root 'plugins') -Directory) {
        $manifest = Get-Content (Join-Path $folder.FullName 'plugin.json') -Raw | ConvertFrom-Json
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
            $entries += [ordered]@{
                id = $manifest.id; name = $manifest.name; version = $manifest.version; apiVersion = $manifest.apiVersion
                description = $manifest.description; category = $manifest.category; icon = $manifest.icon
                permissions = $manifest.permissions
                manifest = "$base/plugin.json"; manifestSignature = (Get-Content (Join-Path $out 'plugin.json.sig') -Raw).Trim()
                wasm = "$base/plugin.wasm"; wasmSignature = (Get-Content (Join-Path $out 'plugin.wasm.sig') -Raw).Trim()
            }
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
