$ErrorActionPreference = 'Stop'

$patchRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$buildRoot = Join-Path $patchRoot 'build'
New-Item -ItemType Directory -Force -Path $buildRoot | Out-Null

$objectPath = Join-Path $buildRoot 'r08_touch_wheel.o'
$elfPath = Join-Path $buildRoot 'r08_touch_wheel.elf'
$binaryPath = Join-Path $buildRoot 'r08_touch_wheel.bin'

rustc `
    --target thumbv6m-none-eabi `
    --crate-type lib `
    --emit "obj=$objectPath" `
    -C panic=abort `
    -C opt-level=z `
    (Join-Path $patchRoot 'src\lib.rs')
if ($LASTEXITCODE -ne 0) { throw "rustc failed with exit code $LASTEXITCODE" }

$rustSysroot = rustc --print sysroot
$linker = Join-Path $rustSysroot 'lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe'
& $linker -flavor gnu -T (Join-Path $patchRoot 'linker.ld') -o $elfPath $objectPath
if ($LASTEXITCODE -ne 0) { throw "ELF link failed with exit code $LASTEXITCODE" }
& $linker -flavor gnu -T (Join-Path $patchRoot 'linker.ld') --oformat=binary -o $binaryPath $objectPath
if ($LASTEXITCODE -ne 0) { throw "binary link failed with exit code $LASTEXITCODE" }

Write-Host "ELF: $elfPath"
Write-Host "BIN: $binaryPath"
Write-Host "Size: $((Get-Item -LiteralPath $binaryPath).Length) bytes"
