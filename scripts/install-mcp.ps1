# Instala Ferrum como servidor MCP para agentes de IA (Claude Code y otros).
#
#   .\scripts\build-windows.ps1     # primero, compila
#   .\scripts\install-mcp.ps1       # copia ferrum.exe a %LOCALAPPDATA%\Programs\Ferrum
#
# Se usa una copia fuera de la carpeta de compilación: mientras un agente tiene
# Ferrum abierto, Windows bloquea su ejecutable y no se podría recompilar.
# Ver README.md, sección «Usar Ferrum desde un agente de IA».

param(
    [string]$CargoProfile = "release"
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
$source = Join-Path $targetDir "$profileDir\ferrum.exe"
if (-not (Test-Path $source)) {
    throw "No existe $source. Compila antes con: .\scripts\build-windows.ps1"
}

$installDir = Join-Path $env:LOCALAPPDATA "Programs\Ferrum"
New-Item -ItemType Directory -Force $installDir | Out-Null
$exe = Join-Path $installDir "ferrum.exe"

# Si un agente está usando la versión anterior, Windows no deja sobrescribirla
# pero sí renombrarla. Las copias viejas se borran cuando ya nadie las usa.
Get-ChildItem $installDir -Filter "ferrum-anterior-*.exe" | ForEach-Object {
    Remove-Item $_.FullName -ErrorAction SilentlyContinue
}
if (Test-Path $exe) {
    $stamp = Get-Date -Format "yyyyMMddHHmmss"
    Rename-Item $exe "ferrum-anterior-$stamp.exe"
}
Copy-Item $source $exe

Write-Host "Ferrum instalado en $exe"
Write-Host ""
Write-Host "Para Claude Code:"
Write-Host "  claude mcp add ferrum -- `"$exe`" --mcp"
Write-Host "O en un archivo .mcp.json:"
Write-Host '  { "mcpServers": { "ferrum": { "command": "${LOCALAPPDATA}\\Programs\\Ferrum\\ferrum.exe", "args": ["--mcp"] } } }'
