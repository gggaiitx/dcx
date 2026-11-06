$magic = [byte[]](0x11,0xf4,0x7a,0xd5,0x7b,0x73,0x42,0xc0,0xab,0xae,0x87,0x8b,0x1e,0x16,0xad,0xee)
function Get-Piid([string]$sig) {
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($sig)
  $all = New-Object byte[] ($magic.Length + $bytes.Length)
  [Array]::Copy($magic, 0, $all, 0, $magic.Length); [Array]::Copy($bytes, 0, $all, $magic.Length, $bytes.Length)
  $h = [System.Security.Cryptography.SHA1]::Create().ComputeHash($all)
  $d1 = ([uint32]$h[0] -shl 24) -bor ([uint32]$h[1] -shl 16) -bor ([uint32]$h[2] -shl 8) -bor [uint32]$h[3]
  $d2 = ([uint16]$h[4] -shl 8) -bor [uint16]$h[5]
  $d3 = ((([uint16]$h[6] -shl 8) -bor [uint16]$h[7]) -band 0x0fff) -bor 0x5000
  $d4 = @(($h[8] -band 0x3f) -bor 0x80) + $h[9..15]
  "{0:X8}-{1:X4}-{2:X4}-{3}" -f $d1, $d2, $d3, (($d4 | ForEach-Object { "{0:X2}" -f $_ }) -join '')
}
$vector = '913337e9-11a1-4345-a3a2-4e7f956e222d'
$map    = '3c2925fe-8519-45c1-aa79-197b6718c1c1'
Write-Host ("IVector<SetterBase>    = " + (Get-Piid "pinterface({$vector};rc(Windows.UI.Xaml.SetterBase;{418be27c-2ac4-4f22-8097-dea3aeeb2fb3}))"))
Write-Host ("IVector<ColumnDef>     = " + (Get-Piid "pinterface({$vector};rc(Windows.UI.Xaml.Controls.ColumnDefinition;{f7f1b229-f024-467f-970a-7e705615db7b}))"))
Write-Host ("IVector<VisualState>   = " + (Get-Piid "pinterface({$vector};rc(Windows.UI.Xaml.VisualState;{6320affc-c31a-4450-afde-f6ea7bd1f586}))"))
Write-Host ("IVector<VSGroup>       = " + (Get-Piid "pinterface({$vector};rc(Windows.UI.Xaml.VisualStateGroup;{e4f9d9a4-e028-44de-9b15-4929ae0a26c2}))"))
Write-Host ("IVectorView<SetterBase> = " + (Get-Piid "pinterface({6a79e863-4300-459a-9966-cbb660963ee1};rc(Windows.UI.Xaml.SetterBase;{418be27c-2ac4-4f22-8097-dea3aeeb2fb3}))"))
Write-Host ("IMap<object,object>    = " + (Get-Piid "pinterface({$map};cinterface(IInspectable);cinterface(IInspectable))"))
