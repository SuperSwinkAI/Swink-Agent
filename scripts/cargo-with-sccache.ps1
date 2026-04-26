param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

$sccache = Get-Command sccache -ErrorAction SilentlyContinue
if ($null -ne $sccache) {
    $env:RUSTC_WRAPPER = "sccache"
} else {
    $env:RUSTC_WRAPPER = ""
}

& cargo @CargoArgs
exit $LASTEXITCODE
