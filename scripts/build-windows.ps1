# Compila Ferrum en Windows.
#
#   .\scripts\build-windows.ps1            # compilación release
#   .\scripts\build-windows.ps1 -Run       # compila y abre el navegador
#   .\scripts\build-windows.ps1 -NoMedia   # sin vídeo ni audio (no hace falta GStreamer)
#   .\scripts\build-windows.ps1 -CargoProfile production   # binario de distribución
#
# Requisitos: Rust (MSVC), Visual Studio 2022 Build Tools (C++, ATL, SDK) y LLVM.
# Para vídeo y audio, además, GStreamer 1.24 o posterior (MSVC x86_64). Si está
# instalado, se activa solo y sus bibliotecas se copian junto a ferrum.exe.
# Ver README.md.

param(
    [string]$CargoProfile = "release",
    [switch]$Run,
    [switch]$NoMedia,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$FerrumArgs
)

$ErrorActionPreference = "Stop"

# Plugins de GStreamer que carga Servo en Windows: la misma lista que
# servo/gstreamer_plugin_lists (common.rs.in y windows.rs.in).
$GStreamerPlugins = @(
    "gstcoreelements", "gstnice", "gstapp", "gstaudioconvert", "gstaudioresample",
    "gstgio", "gstogg", "gstopengl", "gstopus", "gstplayback", "gsttheora",
    "gsttypefindfunctions", "gstvideoconvertscale", "gstvolume", "gstvorbis",
    "gstaudiofx", "gstaudioparsers", "gstautodetect", "gstdeinterlace", "gstid3demux",
    "gstinterleave", "gstisomp4", "gstmatroska", "gstrtp", "gstrtpmanager",
    "gstvideofilter", "gstvpx", "gstwavparse", "gstaudiobuffersplit", "gstdtls",
    "gstid3tag", "gstproxy", "gstvideoparsersbad", "gstwebrtc", "gstlibav", "gstwasapi"
)

function Find-GStreamer {
    $candidates = @(
        $env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
        (Join-Path $env:LOCALAPPDATA "Programs\gstreamer\1.0\msvc_x86_64"),
        "C:\Program Files\gstreamer\1.0\msvc_x86_64",
        "C:\gstreamer\1.0\msvc_x86_64"
    )
    foreach ($root in $candidates) {
        if ($root -and (Test-Path (Join-Path $root "lib\pkgconfig\gstreamer-1.0.pc"))) {
            return (Resolve-Path $root).Path
        }
    }
    return $null
}

# Copia junto a ferrum.exe los plugins y solo las bibliotecas de GStreamer que
# necesitan (siguiendo sus importaciones con llvm-objdump), no la instalación
# entera.
function Copy-GStreamerRuntime([string]$Exe, [string]$Root) {
    $objdump = Join-Path $env:LIBCLANG_PATH "llvm-objdump.exe"
    $bin = Join-Path $Root "bin"
    $pluginDir = Join-Path $Root "lib\gstreamer-1.0"
    $destination = Split-Path $Exe

    $queue = [System.Collections.Generic.Queue[string]]::new()
    $queue.Enqueue($Exe)
    $files = @{}
    foreach ($plugin in $GStreamerPlugins) {
        $path = Join-Path $pluginDir "$plugin.dll"
        if (-not (Test-Path $path)) { throw "Falta el plugin de GStreamer $plugin.dll" }
        $files["$plugin.dll"] = $path
        $queue.Enqueue($path)
    }
    while ($queue.Count -gt 0) {
        $file = $queue.Dequeue()
        foreach ($line in & $objdump -p $file) {
            if ($line -notmatch "DLL Name:\s*(\S+)") { continue }
            $name = $Matches[1]
            $path = Join-Path $bin $name
            if (-not $files.ContainsKey($name) -and (Test-Path $path)) {
                $files[$name] = $path
                $queue.Enqueue($path)
            }
        }
    }

    $bytes = 0
    foreach ($path in $files.Values) {
        Copy-Item $path $destination -Force
        $bytes += (Get-Item $path).Length
    }
    # Lista de lo copiado, para que install-windows.ps1 sepa qué instalar.
    $files.Keys | Sort-Object | Set-Content (Join-Path $destination "gstreamer-files.txt")
    Write-Host ("GStreamer: {0} archivos copiados junto a ferrum.exe ({1:N0} MB)" -f $files.Count, ($bytes / 1MB))
}

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

$features = @()
$gstreamer = $null
if (-not $NoMedia) {
    $gstreamer = Find-GStreamer
    if ($gstreamer) {
        Write-Host "Vídeo y audio con GStreamer: $gstreamer"
        $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = "$gstreamer\"
        $env:PKG_CONFIG = Join-Path $gstreamer "bin\pkg-config.exe"
        $env:PKG_CONFIG_PATH = Join-Path $gstreamer "lib\pkgconfig"
        $env:Path = "$(Join-Path $gstreamer 'bin');$env:Path"
        $features = @("--features", "media")
    } else {
        Write-Warning ("GStreamer no está instalado: Ferrum se compila sin vídeo ni audio. " +
            "Descárgalo de https://gstreamer.freedesktop.org/download/ (MSVC x86_64).")
    }
}

Push-Location "$PSScriptRoot\.."
try {
    cargo build --profile $CargoProfile -p ferrum @features
    if ($LASTEXITCODE -ne 0) { throw "La compilación falló" }

    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
    $profileDir = if ($CargoProfile -eq "dev") { "debug" } else { $CargoProfile }
    $exe = (Resolve-Path (Join-Path $targetDir "$profileDir\ferrum.exe")).Path
    if ($gstreamer) { Copy-GStreamerRuntime $exe $gstreamer }
    Write-Host "Listo: $exe"

    if ($Run) { & $exe @FerrumArgs }
}
finally {
    Pop-Location
}
