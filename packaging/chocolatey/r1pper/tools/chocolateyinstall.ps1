$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$msiPath  = Join-Path $toolsDir 'r1pper.msi'

$packageArgs = @{
  packageName    = 'r1pper'
  fileType       = 'msi'
  file           = $msiPath
  silentArgs     = '/qn /norestart'
  validExitCodes = @(0, 3010, 1641)
}

Install-ChocolateyInstallPackage @packageArgs