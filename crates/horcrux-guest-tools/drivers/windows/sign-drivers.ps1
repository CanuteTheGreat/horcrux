# sign-drivers.ps1 - run by `horcrux-guest-agent --drivers install` (as SYSTEM,
# from the MSI) to make the Horcrux guest drivers installable without test
# mode: a code-signing certificate is generated for THIS guest, its public
# part is trusted (LocalMachine Root + TrustedPublisher), each driver package
# in -Dir gets a catalog signed with it, and the private key is deleted
# right after, so nothing can sign anything else with it later.
# Only inbox cmdlets (PKI, Security modules) are used.
# SPDX-License-Identifier: GPL-3.0-or-later
param(
  [Parameter(Mandatory = $true)][string]$Dir,
  [Parameter(Mandatory = $true)][string]$Subject,
  [int]$KeyLength = 3072,
  [int]$Years = 30
)
$ErrorActionPreference = 'Stop'
$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject `
  -CertStoreLocation Cert:\LocalMachine\My -KeyAlgorithm RSA -KeyLength $KeyLength `
  -HashAlgorithm SHA256 -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddYears($Years)
try {
  $cer = Join-Path $Dir 'horcrux-drivers.cer'
  Export-Certificate -Cert $cert -FilePath $cer | Out-Null
  Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
  Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
  foreach ($pkg in Get-ChildItem -Directory $Dir) {
    $name = $pkg.Name
    foreach ($dll in Get-ChildItem -File (Join-Path $pkg.FullName '*.dll')) {
      $s = Set-AuthenticodeSignature -FilePath $dll.FullName -Certificate $cert -HashAlgorithm SHA256
      if ($s.Status -ne 'Valid') { throw "sign $($dll.Name): $($s.Status) $($s.StatusMessage)" }
    }
    $tmp = Join-Path $Dir "$name.cat"
    Remove-Item -Force -ErrorAction SilentlyContinue $tmp, (Join-Path $pkg.FullName "$name.cat")
    New-FileCatalog -Path $pkg.FullName -CatalogFilePath $tmp -CatalogVersion 2.0 | Out-Null
    Move-Item $tmp (Join-Path $pkg.FullName "$name.cat")
    $s = Set-AuthenticodeSignature -FilePath (Join-Path $pkg.FullName "$name.cat") -Certificate $cert -HashAlgorithm SHA256
    if ($s.Status -ne 'Valid') { throw "sign $name.cat: $($s.Status) $($s.StatusMessage)" }
    Write-Output "SIGNED=$name"
  }
  Write-Output "THUMBPRINT=$($cert.Thumbprint)"
} finally {
  # the private key never outlives this script
  Remove-Item -Path "Cert:\LocalMachine\My\$($cert.Thumbprint)" -DeleteKey -ErrorAction SilentlyContinue
}
