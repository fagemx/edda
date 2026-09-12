param([Parameter(Mandatory=$true)][string]$Path)
$ErrorActionPreference = 'Stop'
$item = Get-Item -LiteralPath $Path -Force
if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Channel storage must be a real directory'
}
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetOwner($sid)
$acl.SetAccessRuleProtection($true, $false)
$rule = New-Object Security.AccessControl.FileSystemAccessRule(
    $sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'
)
$acl.AddAccessRule($rule)
$item.SetAccessControl($acl)
$actual = $item.GetAccessControl()
if (-not $actual.AreAccessRulesProtected) { throw 'Channel ACL still inherits access' }
foreach ($entry in $actual.Access) {
    if ($entry.AccessControlType -eq 'Allow' -and
        $entry.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) {
        throw 'Channel ACL permits another principal'
    }
}
