#Requires -Version 5.1

function New-TalosRandomSecret {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[a-z][a-z0-9_]*$')]
    [string] $Purpose,

    [ValidateRange(32, 128)]
    [int] $ByteCount = 32
  )

  $bytes = New-Object byte[] $ByteCount
  $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
  try {
    $generator.GetBytes($bytes)
  }
  finally {
    $generator.Dispose()
  }

  $base64Url = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
  return "talos_$($Purpose)_$base64Url"
}
