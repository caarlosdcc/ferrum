# Compila Ferrum en Windows.
#
#   .\scripts\build-windows.ps1            # compilación release
#   .\scripts\build-windows.ps1 -Run       # compila y abre el navegador
#   .\scripts\build-windows.ps1 -CargoProfile production   # binario de distribución
#
# Requisitos: Rust (MSVC), Visual Studio 2022 Build Tools (C++, ATL, SDK) y LLVM.
# Ver README.md.

param(
    [string]$CargoProfile = "release",
    [switch]$Run,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$FerrumArgs
)

$ErrorActionPreference = "Stop"

# libclang: Servo lo usa para generar los bindings de SpiderMonkey.
if (-not $env:LIBCLANG_PATH) {
    $llvm = "C:\Program Files\LLVM\bin"
    if (-not (Test-Path "$llvm\libclang.dll")) {
        throw "No se encontró LLVM. Instálalo con: winget install LLVM.LLVM"
    }
    $env:LIBCLANG_PATH = $llvm
}

# La carpeta target ocupa decenas de GB: fuera de OneDrive y de rutas con espacios.
if (-not $env:CARGO_TARGET_DIR) {
    $root = (Resolve-Path "$PSScriptRoot\..").Path
    if ($root -match "OneDrive" -or $root -match " ") {
        $env:CARGO_TARGET_DIR = Join-Path $env:USERPROFILE ".cargo-target\ferrum"
        Write-Host "Usando carpeta de compilación: $env:CARGO_TARGET_DIR"
    }
}

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) {
    $env:Path = "$cargoBin;$env:Path"
}

Push-Location "$PSScriptRoot\.."
try {
    cargo build --profile $CargoProfile -p ferrum
    if ($LASTEXITCODE -ne 0) { throw "La compilación falló" }

    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
    $profileDir = if ($CargoProfile -eq "dev") { "debug" } else { $CargoProfile }
    $exe = Join-Path $targetDir "$profileDir\ferrum.exe"
    Write-Host "Listo: $exe"

    if ($Run) { & $exe @FerrumArgs }
}
finally {
    Pop-Location
}
