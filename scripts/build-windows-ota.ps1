# 由私有 Windows 发布流水线在构建机上调用(仓库内无 CI 引用属预期)。
# 输入产物名与 release-packages.yml 的 tauri nsis 默认名
# pinvou3_<version>_x64-setup.exe 一致。
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
    [string]$SourceExe,

    [Parameter(Position = 1)]
    [string]$OutputPath,

    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-NormalizedVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallerPath
    )

    $fileName = [System.IO.Path]::GetFileName($InstallerPath)
    $match = [regex]::Match(
        $fileName,
        '^pinvou3_(?<version>\d+(?:\.\d+){2,3})_x64-setup\.exe$',
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )

    if ($match.Success) {
        $version = $match.Groups['version'].Value
    } else {
        $version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($InstallerPath).ProductVersion
        if ([string]::IsNullOrWhiteSpace($version)) {
            $version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($InstallerPath).FileVersion
        }
        $versionMatch = [regex]::Match([string]$version, '\d+(?:\.\d+){2,3}')
        if (-not $versionMatch.Success) {
            throw "Cannot determine a version from the installer name or version metadata: $fileName"
        }
        $version = $versionMatch.Value
    }

    $parts = @($version.Split('.'))
    if ($parts.Count -eq 3) {
        return "$version.0"
    }
    if ($parts.Count -eq 4) {
        return $version
    }
    throw "The OTA version must contain three or four numeric components: $version"
}

function Get-FileMd5 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm MD5).Hash.ToLowerInvariant()
}

function ConvertTo-CrlfJson {
    param([Parameter(Mandatory = $true)]$Value)
    $json = $Value | ConvertTo-Json -Depth 12
    return ($json -replace "(?<!`r)`n", "`r`n")
}

function Add-Utf8NoBomEntry {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.Compression.ZipArchive]$Archive,
        [Parameter(Mandatory = $true)]
        [string]$EntryName,
        [Parameter(Mandatory = $true)]
        [string]$Text
    )

    $entry = $Archive.CreateEntry(
        $EntryName,
        [System.IO.Compression.CompressionLevel]::SmallestSize
    )
    $entryStream = $entry.Open()
    try {
        $encoding = [System.Text.UTF8Encoding]::new($false)
        $bytes = $encoding.GetBytes($Text)
        $entryStream.Write($bytes, 0, $bytes.Length)
    } finally {
        $entryStream.Dispose()
    }
}

function Read-ZipEntryBytes {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.Compression.ZipArchive]$Archive,
        [Parameter(Mandatory = $true)]
        [string]$EntryName
    )

    $entry = $Archive.GetEntry($EntryName)
    if ($null -eq $entry) {
        throw "ZIP entry is missing: $EntryName"
    }
    $stream = $entry.Open()
    $memory = [System.IO.MemoryStream]::new()
    try {
        $stream.CopyTo($memory)
        return $memory.ToArray()
    } finally {
        $stream.Dispose()
        $memory.Dispose()
    }
}

function Assert-Utf8NoBomJson {
    param(
        [Parameter(Mandatory = $true)]
        [byte[]]$Bytes,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if ($Bytes.Length -ge 3 -and
        $Bytes[0] -eq 0xEF -and
        $Bytes[1] -eq 0xBB -and
        $Bytes[2] -eq 0xBF) {
        throw "$Label contains a UTF-8 BOM"
    }
    $text = [System.Text.Encoding]::UTF8.GetString($Bytes)
    try {
        return $text | ConvertFrom-Json
    } catch {
        throw "$Label is not valid UTF-8 JSON: $($_.Exception.Message)"
    }
}

$source = (Resolve-Path -LiteralPath $SourceExe).Path
if ([System.IO.Path]::GetExtension($source) -ine '.exe') {
    throw "The source must be an EXE installer: $source"
}

$version = Get-NormalizedVersion -InstallerPath $source
$exeName = [System.IO.Path]::GetFileName($source)
$sourceDirectory = [System.IO.Path]::GetDirectoryName($source)
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $sourceDirectory "Pinvou3_$version.zip"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path (Get-Location).Path $OutputPath
}
$output = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = [System.IO.Path]::GetDirectoryName($output)
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    [System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
}
if (Test-Path -LiteralPath $output) {
    if (-not $Force) {
        throw "The output already exists; pass -Force to replace it: $output"
    }
}

$installerMd5 = Get-FileMd5 -Path $source
$attachData = ConvertTo-CrlfJson ([ordered]@{
    version = $version
    exeName = $exeName
})
$attachData = $attachData -replace "`r?`n", '' -replace '\s{2,}', ''

$otaInfo = [ordered]@{
    softwareName = 'Pinvou3'
    softwareId = 'Pinvou3_Win'
    softwareVersion = $version
    attachData = $null
    sourceDir = $null
    softwareType = 'SoftwareCollection'
    fileMetaInfos = $null
    softwareInfos = @(
        [ordered]@{
            softwareName = 'Pinvou3'
            softwareId = 'Pinvou3_Win'
            softwareVersion = $version
            attachData = $attachData
            sourceDir = 'Pinvou3'
            softwareType = 'Pinvou3'
            fileMetaInfos = @(
                [ordered]@{
                    fileName = $exeName
                    filePath = $exeName
                    hash = $installerMd5
                    ignoreHash = $false
                }
            )
            softwareInfos = $null
        }
    )
}

$workRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("pinvou3-ota-" + [guid]::NewGuid().ToString('N'))
[System.IO.Directory]::CreateDirectory($workRoot) | Out-Null
$workRootResolved = [System.IO.Path]::GetFullPath($workRoot)
$fullPackPath = Join-Path $workRootResolved 'FullPack.zip'
$temporaryOutput = Join-Path $outputDirectory ('.' + [System.IO.Path]::GetFileName($output) + '.tmp-' + [guid]::NewGuid().ToString('N'))

try {
    $innerFile = [System.IO.File]::Open(
        $fullPackPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
    $innerArchive = [System.IO.Compression.ZipArchive]::new(
        $innerFile,
        [System.IO.Compression.ZipArchiveMode]::Create,
        $false
    )
    try {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $innerArchive,
            $source,
            "Files\Pinvou3\$exeName",
            [System.IO.Compression.CompressionLevel]::SmallestSize
        ) | Out-Null
        Add-Utf8NoBomEntry -Archive $innerArchive -EntryName 'OtaInfo.json' -Text (ConvertTo-CrlfJson $otaInfo)
    } finally {
        $innerArchive.Dispose()
        $innerFile.Dispose()
    }

    $fullPackMd5 = Get-FileMd5 -Path $fullPackPath
    $updatePackInfo = [ordered]@{
        appName = 'Pinvou3'
        appId = 'Pinvou3_Win'
        appVersion = $version
        updateInfo = $null
        updateType = 2
        fullPack = [ordered]@{
            fileName = 'FullPack.zip'
            hash = $fullPackMd5
        }
        incrementalPacks = @()
    }

    $outerFile = [System.IO.File]::Open(
        $temporaryOutput,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
    $outerArchive = [System.IO.Compression.ZipArchive]::new(
        $outerFile,
        [System.IO.Compression.ZipArchiveMode]::Create,
        $false
    )
    try {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $outerArchive,
            $fullPackPath,
            'FullPack.zip',
            [System.IO.Compression.CompressionLevel]::SmallestSize
        ) | Out-Null
        Add-Utf8NoBomEntry -Archive $outerArchive -EntryName 'UpdatePackInfo.json' -Text (ConvertTo-CrlfJson $updatePackInfo)
    } finally {
        $outerArchive.Dispose()
        $outerFile.Dispose()
    }

    # Validate both ZIP layers, versions, MD5 values, JSON syntax, and BOM absence.
    $checkInner = [System.IO.Compression.ZipFile]::OpenRead($fullPackPath)
    try {
        $innerNames = @($checkInner.Entries | ForEach-Object { $_.FullName })
        $expectedInstallerEntry = "Files\Pinvou3\$exeName"
        if ($innerNames.Count -ne 2 -or
            $innerNames -notcontains $expectedInstallerEntry -or
            $innerNames -notcontains 'OtaInfo.json') {
            throw "FullPack.zip entries do not match the OTA contract: $($innerNames -join ', ')"
        }
        $checkedOta = Assert-Utf8NoBomJson -Bytes (Read-ZipEntryBytes -Archive $checkInner -EntryName 'OtaInfo.json') -Label 'OtaInfo.json'
        if ($checkedOta.softwareVersion -ne $version -or
            $checkedOta.softwareInfos[0].fileMetaInfos[0].hash -ne $installerMd5) {
            throw 'OtaInfo.json contains a mismatched version or installer MD5'
        }
    } finally {
        $checkInner.Dispose()
    }

    $checkOuter = [System.IO.Compression.ZipFile]::OpenRead($temporaryOutput)
    try {
        $outerNames = @($checkOuter.Entries | ForEach-Object { $_.FullName })
        if ($outerNames.Count -ne 2 -or
            $outerNames -notcontains 'FullPack.zip' -or
            $outerNames -notcontains 'UpdatePackInfo.json') {
            throw "Outer ZIP entries do not match the OTA contract: $($outerNames -join ', ')"
        }
        $checkedUpdate = Assert-Utf8NoBomJson -Bytes (Read-ZipEntryBytes -Archive $checkOuter -EntryName 'UpdatePackInfo.json') -Label 'UpdatePackInfo.json'
        if ($checkedUpdate.appVersion -ne $version -or
            $checkedUpdate.fullPack.hash -ne $fullPackMd5) {
            throw 'UpdatePackInfo.json contains a mismatched version or FullPack.zip MD5'
        }
    } finally {
        $checkOuter.Dispose()
    }

    if (Test-Path -LiteralPath $output) {
        Remove-Item -LiteralPath $output -Force
    }
    Move-Item -LiteralPath $temporaryOutput -Destination $output

    $package = Get-Item -LiteralPath $output
    [pscustomobject]@{
        PackagePath = $package.FullName
        Version = $version
        InstallerName = $exeName
        InstallerMd5 = $installerMd5
        FullPackMd5 = $fullPackMd5
        PackageMd5 = Get-FileMd5 -Path $output
        PackageSize = $package.Length
        JsonEncoding = 'UTF-8 without BOM'
    }
} finally {
    if (Test-Path -LiteralPath $temporaryOutput) {
        Remove-Item -LiteralPath $temporaryOutput -Force
    }
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if ($workRootResolved.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path $workRootResolved -Leaf) -like 'pinvou3-ota-*' -and
        (Test-Path -LiteralPath $workRootResolved)) {
        Remove-Item -LiteralPath $workRootResolved -Recurse -Force
    }
}
