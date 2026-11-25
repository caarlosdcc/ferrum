# Instala Ferrum para tu usuario en %LOCALAPPDATA%\Programs\Ferrum: el .exe y,
# si se compiló con vídeo y audio, las bibliotecas de GStreamer.
#
#   .\scripts\build-windows.ps1              # primero, compila
#   .\scripts\install-windows.ps1            # instala
#   .\scripts\install-windows.ps1 -Desktop   # y pone un acceso directo en el escritorio
#
# Los agentes de IA usan esta copia por MCP (ferrum.exe --mcp). Al estar fuera
# de la carpeta de compilación, se puede recompilar mientras un agente tiene
# Ferrum abierto. Ver README.md.

param(
    [string]$CargoProfile = "release",
    [switch]$Desktop
)

$ErrorActionPreference = "Stop"

$targetDir = $env:CARGO_TARGET_DIR
if (-not $targetDir) {
    $root = (Resolve-Path "$PSScriptRoot\..").Path
    # Misma regla que build-windows.ps1 para proyectos en OneDrive o con espacios.
    if ($root -match "OneDrive" -or $root -match " ") {
        $targetDir = Join-Path $env:USERPROFILE ".cargo-target\ferrum"
    } else {
        $targetDir = Join-Path $root "target"
    }
}
$profileDir = if ($CargoProfile -eq "dev") { "debug" } else { $CargoProfile }
$buildDir = Join-Path $targetDir $profileDir
$source = Join-Path $buildDir "ferrum.exe"
if (-not (Test-Path $source)) {
    throw "No existe $source. Compila antes con: .\scripts\build-windows.ps1"
}

$installDir = Join-Path $env:LOCALAPPDATA "Programs\Ferrum"
New-Item -ItemType Directory -Force $installDir | Out-Null
$stamp = Get-Date -Format "yyyyMMddHHmmss"

# Restos de instalaciones anteriores que ya nadie usa.
Get-ChildItem $installDir -Filter "*.anterior-*" | ForEach-Object {
    Remove-Item $_.FullName -ErrorAction SilentlyContinue
}

# Si un Ferrum abierto usa el archivo, Windows no deja sobrescribirlo pero sí
# renombrarlo: el viejo sigue funcionando y el nuevo ocupa su sitio.
function Install-File([string]$Path) {
    $target = Join-Path $installDir (Split-Path $Path -Leaf)
    if (Test-Path $target) {
        try {
            Copy-Item $Path $target -Force
            return
        } catch {
            Rename-Item $target ("{0}.anterior-{1}" -f (Split-Path $target -Leaf), $stamp)
        }
    }
    Copy-Item $Path $target
}

Install-File $source
$list = Join-Path $buildDir "gstreamer-files.txt"
$media = Test-Path $list
if ($media) {
    $files = Get-Content $list
    foreach ($name in $files) { Install-File (Join-Path $buildDir $name) }
    Install-File $list
} else {
    Remove-Item (Join-Path $installDir "gstreamer-files.txt") -ErrorAction SilentlyContinue
}
$exe = Join-Path $installDir "ferrum.exe"

if ($Desktop) {
    $desktopDir = [Environment]::GetFolderPath("Desktop")
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut((Join-Path $desktopDir "Ferrum.lnk"))
    $link.TargetPath = $exe
    $link.WorkingDirectory = $installDir
    $link.Description = "Ferrum: navegador ligero, seguro y privado"
    $link.Save()
    Write-Host "Acceso directo creado en el escritorio."
}

if ($media) {
    Write-Host "Ferrum instalado en $exe (con vídeo y audio: $($files.Count) bibliotecas de GStreamer)"
} else {
    Write-Host "Ferrum instalado en $exe (sin vídeo ni audio)"
}
Write-Host ""
Write-Host "Para usarlo desde Claude Code (MCP):"
Write-Host "  claude mcp add ferrum -- `"$exe`" --mcp"
Write-Host "O en un archivo .mcp.json:"
Write-Host '  { "mcpServers": { "ferrum": { "command": "${LOCALAPPDATA}\\Programs\\Ferrum\\ferrum.exe", "args": ["--mcp"] } } }'
