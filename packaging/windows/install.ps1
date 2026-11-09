#Requires -Version 5.1
# Builds the release binary, copies it into the user's bin folder so `tern`
# runs this checkout's build, and adds a Start menu shortcut with Tern's
# icon. Re-run after pulling to update.
# Usage: pwsh -File packaging/windows/install.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..\..')

$binDir = Join-Path $HOME '.local\bin'
$exe = Join-Path $binDir 'tern.exe'
$dataDir = Join-Path $env:LOCALAPPDATA 'tern'
$icon = Join-Path $dataDir 'tern.ico'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Tern.lnk'

cargo build --locked --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Never replace a shortcut we didn't write: that's likely an installed Tern.
$shell = New-Object -ComObject WScript.Shell
if ((Test-Path $shortcut) -and $shell.CreateShortcut($shortcut).TargetPath -ne $exe) {
    Write-Error "$shortcut doesn't point at $exe; remove it first."
}

# A copy, not a symlink: symlinks need Developer Mode, and a linked binary
# would make `cargo build` fail while this Tern is running. The copy is the
# one that's locked instead, so it must be closed to update. Never kill it:
# its shells may be the ones running this script.
if (Get-Process -Name tern -ErrorAction SilentlyContinue | Where-Object Path -eq $exe) {
    Write-Error "$exe is running; close it and re-run."
}
New-Item -ItemType Directory -Force $binDir, $dataDir | Out-Null
Copy-Item target\release\tern.exe $exe -Force
Write-Host "Copied target\release\tern.exe -> $exe"

# Shortcuts take their icon from an .ico. Windows reads a PNG stored in one
# as is, so wrap a 256 px copy of the icon in a one-entry icon directory.
Add-Type -AssemblyName System.Drawing
$source = [Drawing.Image]::FromFile((Resolve-Path assets\icon.png))
$bitmap = New-Object Drawing.Bitmap $source, 256, 256
$png = New-Object IO.MemoryStream
$bitmap.Save($png, [Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose(); $source.Dispose()
$writer = New-Object IO.BinaryWriter ([IO.File]::Create($icon))
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]1)  # header: icon, 1 image
$writer.Write([byte[]](0, 0, 0, 0))                                           # 256x256, no palette
$writer.Write([uint16]1); $writer.Write([uint16]32)                           # planes, bits per pixel
$writer.Write([uint32]$png.Length); $writer.Write([uint32]22)                 # size, offset
$writer.Write($png.ToArray())
$writer.Dispose()

$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.IconLocation = "$icon,0"
$link.Save()
Write-Host "Installed the Start menu shortcut and icon in $dataDir"

$onPath = ($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $binDir }
if (-not $onPath) { Write-Warning "$binDir isn't on your PATH." }
