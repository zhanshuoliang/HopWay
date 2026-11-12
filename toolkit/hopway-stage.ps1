# HopWay invokes this helper only on disposable copies under the current user's
# temporary directory. It never accepts the real client configuration root.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$InformationPreference = 'SilentlyContinue'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
$stageRoot = [IO.Path]::GetFullPath([string]$request.root).TrimEnd('\') + '\'
if (-not $stageRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { exit 2 }

. (Join-Path $PSScriptRoot 'sync-proxy.ps1') -LibraryOnly
$VergeRoot = Join-Path $stageRoot 'verge'
$ProxyName = [string]$request.proxyName
$StaticGroup = [string]$request.staticGroup
$RelayGroup = ''
$DryRun = $false
$MatchToStatic = $true
$LegacyRelayRules = $false
$ProviderUrl = $null
$BaseConfig = $null
$StaticServer = $null
$StaticUser = $null
$StaticPass = $null

try {
    if ($request.action -eq 'repair-verge') {
        $layers = Get-VergeLayers
        foreach ($kind in 'rules', 'groups', 'proxies') {
            $path = $layers[$kind]
            if (-not $path -or -not (Test-Path -LiteralPath $path)) { throw 'missing layer' }
            $bom = Get-BomBytes $path
            $lines = Read-Lines $path
            $blocksWithData = 0
            for ($i = 0; $i -lt $lines.Count; $i++) {
                if ($lines[$i] -notmatch '^prepend:') { continue }
                if ($lines[$i] -match '^prepend:\s*\[\s*\]\s*$') { continue }
                $hasData = $lines[$i] -notmatch '^prepend:\s*(#.*)?$'
                for ($j = $i + 1; $j -lt $lines.Count -and $lines[$j] -notmatch '^\S'; $j++) {
                    if ($lines[$j].Trim() -and $lines[$j].Trim() -notmatch '^#') { $hasData = $true }
                }
                if ($hasData) { $blocksWithData++ }
            }
            if ($blocksWithData -gt 1) { throw 'ambiguous duplicate prepend' }
            $list = Ensure-PrependAnchor $lines
            Save-Text $path ($list -join [Environment]::NewLine) $bom
        }
    } elseif ($request.action -eq 'sync-verge') {
        Sync-Verge
    } else { throw 'invalid staging operation' }
    exit 0
} catch {
    # Paths, config fragments and secrets are deliberately not printed.
    exit 1
}
