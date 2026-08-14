[CmdletBinding()]
param(
    [switch]$TunnelOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RemoteHost = '100.89.138.31'
$RemoteUser = 'minterdeploy'
$Port = 3021
$TaskName = 'Minter noVNC over Tailscale'
$FirewallName = 'Minter noVNC via Tailscale 3021'
$KeyPath = Join-Path $env:USERPROFILE '.ssh\codex_minter_vps_ed25519'
$TailscaleExe = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
$SshExe = Join-Path $env:WINDIR 'System32\OpenSSH\ssh.exe'

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-Prerequisites {
    if (-not (Test-Path -LiteralPath $TailscaleExe -PathType Leaf)) {
        throw "Tailscale CLI not found: $TailscaleExe"
    }
    if (-not (Test-Path -LiteralPath $SshExe -PathType Leaf)) {
        $sshCommand = Get-Command ssh.exe -ErrorAction Stop
        $script:SshExe = $sshCommand.Source
    }
    if (-not (Test-Path -LiteralPath $KeyPath -PathType Leaf)) {
        throw "SSH key not found: $KeyPath"
    }
}

function Get-TailscaleIPv4 {
    foreach ($attempt in 1..30) {
        $lines = @(& $TailscaleExe ip -4 2>$null)
        $candidate = if ($lines.Count) { [string]$lines[0] } else { '' }
        $candidate = $candidate.Trim()
        $parsed = $null
        if ($candidate -and [Net.IPAddress]::TryParse($candidate, [ref]$parsed)) {
            return $candidate
        }
        Start-Sleep -Seconds 1
    }
    throw 'Tailscale has no IPv4 address. Connect Tailscale and run the script again.'
}

function Invoke-Tunnel {
    Assert-Prerequisites
    $tailscaleIp = Get-TailscaleIPv4
    $forward = "${tailscaleIp}:${Port}:127.0.0.1:${Port}"

    & $SshExe `
        -N `
        -T `
        -g `
        -i $KeyPath `
        -o BatchMode=yes `
        -o ConnectTimeout=10 `
        -o ExitOnForwardFailure=yes `
        -o ServerAliveInterval=20 `
        -o ServerAliveCountMax=3 `
        -o StrictHostKeyChecking=accept-new `
        -L $forward `
        "${RemoteUser}@${RemoteHost}"

    exit $LASTEXITCODE
}

if ($TunnelOnly) {
    Invoke-Tunnel
}

if (-not (Test-IsAdministrator)) {
    throw 'Run PowerShell as Administrator, then launch this script again.'
}

Assert-Prerequisites
$tailscaleIp = Get-TailscaleIPv4

Write-Host "Tailscale IP: $tailscaleIp"
Write-Host "Checking VPS noVNC over SSH..."
& $SshExe `
    -T `
    -i $KeyPath `
    -o BatchMode=yes `
    -o ConnectTimeout=10 `
    -o StrictHostKeyChecking=accept-new `
    "${RemoteUser}@${RemoteHost}" `
    'curl -fsS -o /dev/null http://127.0.0.1:3021/vnc.html'
if ($LASTEXITCODE -ne 0) {
    throw "The VPS or noVNC endpoint is unavailable (ssh exit code $LASTEXITCODE)."
}

$existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existingTask) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
}

$listener = Get-NetTCPConnection -LocalAddress $tailscaleIp -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
if ($listener) {
    $owner = Get-CimInstance Win32_Process -Filter "ProcessId=$($listener[0].OwningProcess)" -ErrorAction SilentlyContinue
    $details = if ($owner) { "$($owner.Name) (PID $($owner.ProcessId))" } else { "PID $($listener[0].OwningProcess)" }
    throw "${tailscaleIp}:${Port} is already occupied by $details. It was not stopped."
}

$firewallRule = Get-NetFirewallRule -DisplayName $FirewallName -ErrorAction SilentlyContinue
if (-not $firewallRule) {
    $firewallRule = New-NetFirewallRule `
        -DisplayName $FirewallName `
        -Description 'Allow Minter noVNC only through the laptop Tailscale interface.' `
        -Direction Inbound `
        -Action Allow `
        -Enabled True `
        -Profile Any `
        -Protocol TCP `
        -LocalAddress $tailscaleIp `
        -LocalPort $Port `
        -RemoteAddress '100.64.0.0/10'
} else {
    $firewallRule | Set-NetFirewallRule -Enabled True -Direction Inbound -Action Allow -Profile Any
    $firewallRule | Get-NetFirewallPortFilter | Set-NetFirewallPortFilter -Protocol TCP -LocalPort $Port
    $firewallRule | Get-NetFirewallAddressFilter | Set-NetFirewallAddressFilter `
        -LocalAddress $tailscaleIp `
        -RemoteAddress '100.64.0.0/10'
}

$currentUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$powershellExe = (Get-Command powershell.exe -ErrorAction Stop).Source
$actionArguments = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$PSCommandPath`" -TunnelOnly"
$action = New-ScheduledTaskAction -Execute $powershellExe -Argument $actionArguments
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $currentUser
$principal = New-ScheduledTaskPrincipal -UserId $currentUser -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 20 `
    -RestartInterval (New-TimeSpan -Minutes 1)

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Principal $principal `
    -Settings $settings `
    -Description 'Persistent SSH tunnel from the laptop Tailscale IP to Minter noVNC on the VPS.' `
    -Force | Out-Null

Start-ScheduledTask -TaskName $TaskName

$ready = $false
foreach ($attempt in 1..20) {
    Start-Sleep -Seconds 1
    if (Get-NetTCPConnection -LocalAddress $tailscaleIp -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
        $ready = $true
        break
    }
}
if (-not $ready) {
    $info = Get-ScheduledTaskInfo -TaskName $TaskName
    throw "SSH tunnel did not start. Scheduled task result: $($info.LastTaskResult)."
}

$url = "http://${tailscaleIp}:${Port}/vnc.html?autoconnect=true&resize=scale"
$response = Invoke-WebRequest -UseBasicParsing -Uri $url -TimeoutSec 10
if ($response.StatusCode -ne 200) {
    throw "Tunnel is listening, but noVNC returned HTTP $($response.StatusCode)."
}

Write-Host ''
Write-Host 'Minter noVNC tunnel is ready.' -ForegroundColor Green
Write-Host "Open from any allowed device in your tailnet: $url"
Write-Host "Scheduled task: $TaskName"
Write-Host "Firewall rule:  $FirewallName"

