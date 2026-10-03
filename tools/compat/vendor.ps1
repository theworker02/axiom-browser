# Re-vendors the pinned conformance-test snapshots used by `axiom-compat`.
#
# CI never runs this: the vendored files are committed so every suite runs offline.
# To move a pin, change the commit below, run this script, then run
# `cargo run -p axiom-compat -- all --update` and review the expectation diff.
#
#   powershell -ExecutionPolicy Bypass -File tools/compat/vendor.ps1

$ErrorActionPreference = "Stop"

$Html5libCommit = "224991ec10db04f056a89eed8b0bd8695fd2950e"
$WptCommit = "f085a1efc1f58fbe263d384b1e335d656fe58e66"

# WPT paths copied verbatim (directories are recursive). Python handlers are skipped:
# the local server does not execute wptserve handlers.
$WptPaths = @(
    "LICENSE.md",
    "resources/testharness.js",
    "resources/testharnessreport.js",
    "common",
    "fonts/Ahem.ttf",
    "fonts/ahem.css",
    "images/green-256x256.png",
    "css/reference",
    "css/CSS2/reference",
    "css/CSS2/floats-clear/floats-124-ref.xht",
    "css/CSS2/floats-clear/floats-125-ref.xht",
    "css/CSS2/floats-clear/margin-collapse-clear-002-ref.xht",
    "html/syntax/parsing/resources",
    "html/syntax/parsing/crashtests",
    "dom/nodes",
    "dom/events",
    "dom/collections",
    "dom/lists",
    "css/CSS2/colors",
    "css/CSS2/box-display",
    "css/CSS2/margin-padding-clear",
    "css/css-display",
    "css/css-backgrounds/crashtests",
    "css/css-color/crashtests",
    "css/css-flexbox/crashtests"
)

$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Work = Join-Path ([System.IO.Path]::GetTempPath()) "axiom-vendor-pinned"
New-Item -ItemType Directory -Force $Work | Out-Null

function Fetch-Pinned($url, $dir, $commit, $sparse) {
    if (!(Test-Path $dir)) {
        git init -q $dir
        git -C $dir remote add origin $url
    }
    if ($sparse) {
        git -C $dir sparse-checkout init --no-cone
        git -C $dir sparse-checkout set @($sparse | ForEach-Object { "/$_" })
    }
    git -C $dir fetch -q --depth 1 --filter=blob:none origin $commit
    git -C $dir -c core.autocrlf=false checkout -q --force FETCH_HEAD
}

$h5 = Join-Path $Work "html5lib-tests"
Fetch-Pinned "https://github.com/html5lib/html5lib-tests" $h5 $Html5libCommit @("LICENSE", "tokenizer")
$wpt = Join-Path $Work "wpt"
Fetch-Pinned "https://github.com/web-platform-tests/wpt" $wpt $WptCommit $WptPaths

$h5Out = Join-Path $Root "tests\html5lib-tests"
$wptOut = Join-Path $Root "tests\wpt"
foreach ($out in @($h5Out, $wptOut)) {
    if (Test-Path $out) { Get-ChildItem $out -Force -Exclude "VENDOR.md", ".gitattributes" | Remove-Item -Recurse -Force }
    New-Item -ItemType Directory -Force $out | Out-Null
}

New-Item -ItemType Directory -Force (Join-Path $h5Out "tokenizer") | Out-Null
Copy-Item (Join-Path $h5 "LICENSE") $h5Out
Get-ChildItem (Join-Path $h5 "tokenizer") -Filter *.test | Copy-Item -Destination (Join-Path $h5Out "tokenizer")

foreach ($p in $WptPaths) {
    $src = Join-Path $wpt $p
    $dst = Join-Path $wptOut $p
    New-Item -ItemType Directory -Force (Split-Path $dst) | Out-Null
    if (Test-Path $src -PathType Container) {
        Copy-Item $src $dst -Recurse -Force
    } else {
        Copy-Item $src $dst -Force
    }
}
Get-ChildItem $wptOut -Recurse -Include *.py, __init__.py | Remove-Item -Force

Write-Host "html5lib-tests @ $Html5libCommit -> $h5Out"
Write-Host "wpt @ $WptCommit -> $wptOut"
