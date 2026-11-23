param([Parameter(Mandatory = $true)][string]$Output)
$ErrorActionPreference = 'Stop'
$installer = Resolve-Path (Join-Path $PSScriptRoot '../crates/desktop/installer')
$out = (Resolve-Path $Output).Path
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$vs) { throw 'MSVC x86 build tools are required to build the NSIS appearance library.' }
$vcvars = Join-Path $vs 'VC/Auxiliary/Build/vcvars32.bat'
$command = 'call "{0}" >nul && cl /nologo /LD /O2 /MT /Fo:"{1}\skin.obj" /Fe:"{1}\skin.dll" "{2}\skin.c" user32.lib gdi32.lib comctl32.lib' -f $vcvars, $out, $installer
& cmd.exe /d /c $command
exit $LASTEXITCODE
