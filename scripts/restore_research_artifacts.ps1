[CmdletBinding()]
param(
    [string]$OutputDirectory = ''
)

$ErrorActionPreference = 'Stop'
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactRoot = Join-Path $repositoryRoot 'research_artifacts'
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot 'tmp\restored-research-artifacts'
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputDirectory)

if ($resolvedOutput -eq $repositoryRoot -or $resolvedOutput -eq $artifactRoot) {
    throw 'Output directory cannot be the repository root or research_artifacts.'
}

[System.IO.Directory]::CreateDirectory($resolvedOutput) | Out-Null

$archives = @(
    @{
        Name = 'qring-guest-analysis-20260826.apk'
        PartsDirectory = Join-Path $artifactRoot 'qring_apk'
        ExpectedSha256 = '8b8f60209ba3dde47d803bdfc5852d951efc91377b6eff85ca110cc3ba4d1ddf'
        ExpectedLength = 131757887
    },
    @{
        Name = 'qring-guest-smali-20260826.zip'
        PartsDirectory = Join-Path $artifactRoot 'qring_decompiled'
        ExpectedSha256 = 'e06be7453ed6027388e392b3dfb2a13219522abc35fc3e53fd0ba9a51725dc53'
        ExpectedLength = 139332010
    }
)

foreach ($archive in $archives) {
    $destination = Join-Path $resolvedOutput $archive.Name
    if (Test-Path -LiteralPath $destination) {
        throw "Refusing to overwrite existing file: $destination"
    }

    $parts = @(Get-ChildItem -LiteralPath $archive.PartsDirectory -Filter "$($archive.Name).part*" -File | Sort-Object Name)
    if ($parts.Count -eq 0) {
        throw "No parts found: $($archive.PartsDirectory)"
    }

    $output = [System.IO.File]::Create($destination)
    try {
        foreach ($part in $parts) {
            $input = [System.IO.File]::OpenRead($part.FullName)
            try {
                $input.CopyTo($output)
            }
            finally {
                $input.Dispose()
            }
        }
    }
    finally {
        $output.Dispose()
    }

    $item = Get-Item -LiteralPath $destination
    $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToLowerInvariant()
    if ($item.Length -ne $archive.ExpectedLength -or $sha256 -ne $archive.ExpectedSha256) {
        throw "Restored file verification failed: $destination"
    }

    Write-Output "RESTORED $destination"
    Write-Output "SHA256   $sha256"
}

Write-Output 'All split archives were restored and passed length and SHA-256 checks.'
Write-Output 'The script does not install the APK or flash the ring.'
