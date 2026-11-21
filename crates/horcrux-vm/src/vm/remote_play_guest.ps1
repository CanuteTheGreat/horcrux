# Horcrux Remote Play guest setup (runs as SYSTEM through the QEMU guest
# agent; docs/design/gaming-vm.md section 16). Options and secrets arrive as
# JSON on stdin, never on the command line. Prints one line
# "HXRP:<json report>" at the end.
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'
$in = [Console]::In.ReadToEnd() | ConvertFrom-Json
$msgs = New-Object System.Collections.Generic.List[string]

function Get-SteamPath {
    foreach ($k in 'HKLM:\SOFTWARE\WOW6432Node\Valve\Steam', 'HKLM:\SOFTWARE\Valve\Steam') {
        $p = (Get-ItemProperty -Path $k -Name InstallPath -ErrorAction SilentlyContinue).InstallPath
        if ($p -and (Test-Path (Join-Path $p 'steam.exe'))) { return $p }
    }
    $d = Join-Path ${env:ProgramFiles(x86)} 'Steam'
    if (Test-Path (Join-Path $d 'steam.exe')) { return $d }
    return $null
}

# ---- Steam, downloaded from Valve and checked before it runs
if ($in.install_steam -and -not (Get-SteamPath)) {
    $tmp = Join-Path $env:TEMP 'HorcruxSteamSetup.exe'
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $in.steam_url -OutFile $tmp
        $sig = Get-AuthenticodeSignature -FilePath $tmp
        if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Valve') {
            $msgs.Add("Steam installer not run: its signature is $($sig.Status) ($($sig.SignerCertificate.Subject))")
        } else {
            $p = Start-Process -FilePath $tmp -ArgumentList '/S' -Wait -PassThru
            $msgs.Add("Steam installer finished (exit code $($p.ExitCode))")
        }
    } catch {
        $msgs.Add("Steam download failed: $($_.Exception.Message)")
    } finally {
        Remove-Item -Path $tmp -Force -ErrorAction SilentlyContinue
    }
}

# ---- automatic logon: the password goes into the LSA secret
# DefaultPassword (what Winlogon reads), never into the registry in clear
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HxLsa {
    [StructLayout(LayoutKind.Sequential)]
    struct LSA_UNICODE_STRING { public ushort Length; public ushort MaximumLength; public IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)]
    struct LSA_OBJECT_ATTRIBUTES { public int Length; public IntPtr RootDirectory; public IntPtr ObjectName; public uint Attributes; public IntPtr SecurityDescriptor; public IntPtr SecurityQualityOfService; }
    [DllImport("advapi32.dll")] static extern uint LsaOpenPolicy(IntPtr SystemName, ref LSA_OBJECT_ATTRIBUTES ObjectAttributes, uint DesiredAccess, out IntPtr PolicyHandle);
    [DllImport("advapi32.dll")] static extern uint LsaStorePrivateData(IntPtr PolicyHandle, ref LSA_UNICODE_STRING KeyName, IntPtr PrivateData);
    [DllImport("advapi32.dll")] static extern uint LsaStorePrivateData(IntPtr PolicyHandle, ref LSA_UNICODE_STRING KeyName, ref LSA_UNICODE_STRING PrivateData);
    [DllImport("advapi32.dll")] static extern uint LsaClose(IntPtr PolicyHandle);
    [DllImport("advapi32.dll")] static extern uint LsaNtStatusToWinError(uint Status);
    static LSA_UNICODE_STRING Str(string s) {
        LSA_UNICODE_STRING u = new LSA_UNICODE_STRING();
        u.Buffer = Marshal.StringToHGlobalUni(s);
        u.Length = (ushort)(s.Length * 2);
        u.MaximumLength = (ushort)(s.Length * 2 + 2);
        return u;
    }
    public static uint Store(string key, string value) {
        LSA_OBJECT_ATTRIBUTES oa = new LSA_OBJECT_ATTRIBUTES();
        oa.Length = Marshal.SizeOf(oa);
        IntPtr h;
        uint r = LsaOpenPolicy(IntPtr.Zero, ref oa, 0x000F0FFF, out h);
        if (r != 0) return LsaNtStatusToWinError(r);
        LSA_UNICODE_STRING k = Str(key);
        try {
            if (value == null) {
                r = LsaStorePrivateData(h, ref k, IntPtr.Zero);
            } else {
                LSA_UNICODE_STRING v = Str(value);
                r = LsaStorePrivateData(h, ref k, ref v);
                Marshal.ZeroFreeGlobalAllocUnicode(v.Buffer);
            }
        } finally {
            Marshal.FreeHGlobal(k.Buffer);
            LsaClose(h);
        }
        return LsaNtStatusToWinError(r);
    }
}
'@
$wl = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
if ($in.auto_login_user) {
    $user = [string]$in.auto_login_user
    $ok = $false
    try {
        Add-Type -AssemblyName System.DirectoryServices.AccountManagement
        $ctx = New-Object System.DirectoryServices.AccountManagement.PrincipalContext('Machine')
        $ok = $ctx.ValidateCredentials($user, [string]$in.auto_login_password)
    } catch { $msgs.Add("could not check the password: $($_.Exception.Message)") }
    if (-not $ok) {
        $msgs.Add("automatic logon not changed: the user name or password is wrong")
    } else {
        $r = [HxLsa]::Store('DefaultPassword', [string]$in.auto_login_password)
        if ($r -ne 0) {
            $msgs.Add("automatic logon not changed: LsaStorePrivateData error $r")
        } else {
            Set-ItemProperty -Path $wl -Name AutoAdminLogon -Value '1'
            Set-ItemProperty -Path $wl -Name DefaultUserName -Value $user
            Set-ItemProperty -Path $wl -Name DefaultDomainName -Value $env:COMPUTERNAME
            Remove-ItemProperty -Path $wl -Name DefaultPassword -ErrorAction SilentlyContinue
            Remove-ItemProperty -Path $wl -Name AutoLogonCount -ErrorAction SilentlyContinue
            $msgs.Add("automatic logon on for $user")
        }
    }
}
if ($in.auto_login_off) {
    Set-ItemProperty -Path $wl -Name AutoAdminLogon -Value '0'
    [void][HxLsa]::Store('DefaultPassword', $null)
    Remove-ItemProperty -Path $wl -Name DefaultPassword -ErrorAction SilentlyContinue
    $msgs.Add("automatic logon off")
}
$in.auto_login_password = $null

# ---- Steam opens in Big Picture at every logon (all users)
$startup = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\StartUp'
$bp = Join-Path $startup 'Steam Big Picture (Horcrux).url'
if ($in.big_picture) {
    Set-Content -Path $bp -Value "[InternetShortcut]`r`nURL=steam://open/bigpicture`r`n" -Encoding ASCII
} else {
    Remove-Item -Path $bp -Force -ErrorAction SilentlyContinue
}

# ---- no sleep or hibernate on a timer (a sleeping guest can't answer)
if ($in.keep_awake) {
    foreach ($a in 'standby-timeout-ac', 'standby-timeout-dc', 'hibernate-timeout-ac', 'hibernate-timeout-dc') {
        powercfg /change $a 0 | Out-Null
    }
}
$standby = (powercfg /query SCHEME_CURRENT SUB_SLEEP STANDBYIDLE) -join "`n"
$hib = (powercfg /query SCHEME_CURRENT SUB_SLEEP HIBERNATEIDLE) -join "`n"
$awake = ($standby -match 'Current AC Power Setting Index: 0x00000000') -and ($hib -notmatch 'Current AC Power Setting Index: 0x0*[1-9a-f]')

# ---- report
$steam = Get-SteamPath
$prof = ''
try { $prof = [string]((Get-NetConnectionProfile -ErrorAction Stop | Select-Object -First 1).NetworkCategory) } catch {}
$fw = $false
try { $fw = [bool](Get-NetFirewallApplicationFilter -ErrorAction Stop | Where-Object { $_.Program -like '*steam*' } | Select-Object -First 1) } catch {}
$sun = [bool](Get-Service -Name 'SunshineService' -ErrorAction SilentlyContinue) -or (Test-Path (Join-Path $env:ProgramFiles 'Sunshine\sunshine.exe'))
$wlv = Get-ItemProperty -Path $wl -ErrorAction SilentlyContinue
$report = [ordered]@{
    os = 'windows'
    steam_installed = [bool]$steam
    steam_running = [bool](Get-Process -Name 'steam' -ErrorAction SilentlyContinue)
    sunshine_installed = [bool]$sun
    auto_login = ($wlv.AutoAdminLogon -eq '1')
    auto_login_user = [string]$wlv.DefaultUserName
    big_picture = (Test-Path $bp)
    keep_awake = [bool]$awake
    network_profile = $prof
    steam_firewall_rules = [bool]$fw
    checked_at = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    messages = @($msgs)
}
Write-Output ('HXRP:' + ($report | ConvertTo-Json -Compress))
