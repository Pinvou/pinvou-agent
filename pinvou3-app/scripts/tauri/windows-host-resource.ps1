# Build-only Windows SDK adapter. Never installed or callable through host IPC.
param(
  [Parameter(Mandatory=$true)][ValidateSet('Embed','Read','VerifyImage','Inventory')][string]$Mode,
  [string]$Image,
  [string]$Manifest,
  [string]$ExpectedSigner,
  [string]$Inventory
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
  Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class PinvouHostBuildResource {
  // This closed private format uses only nonnegative safe integer numbers.
  // Check its canonical framing before PowerShell's object projection, whose
  // JSON conversion otherwise hides duplicate keys. The original JS V1 codec
  // remains the sole builder encoder; no public protocol parser is replaced.
  sealed class PrivateJsonFraming {
    readonly string text;int position;
    readonly System.Text.UTF8Encoding utf8=new System.Text.UTF8Encoding(false,true);
    public PrivateJsonFraming(byte[] bytes){text=utf8.GetString(bytes);}
    static void Invalid(){throw new InvalidOperationException();}
    char Next(){if(position>=text.Length)Invalid();return text[position++];}
    string String() {
      if(Next()!='"')Invalid();var value=new System.Text.StringBuilder();
      for(;;) {
        char ch=Next();if(ch=='"')break;
        if(ch<32)Invalid();
        if(ch=='\\') {
          char escape=Next();
          switch(escape) {
            case '"':ch='"';break;case '\\':ch='\\';break;
            case 'b':ch='\b';break;case 't':ch='\t';break;case 'n':ch='\n';break;
            case 'f':ch='\f';break;case 'r':ch='\r';break;
            case 'u':
              if(Next()!='0'||Next()!='0')Invalid();
              char high=Next(),low=Next();
              if(high<'0'||high>'1'||!((low>='0'&&low<='9')||(low>='a'&&low<='f')))Invalid();
              ch=(char)((high-'0')*16+(low<='9'?low-'0':low-'a'+10));
              if(ch==8||ch==9||ch==10||ch==12||ch==13)Invalid();
              break;
            default:Invalid();break;
          }
        }
        value.Append(ch);
      }
      string result=value.ToString();if(utf8.GetByteCount(result)>65536)Invalid();return result;
    }
    void Value(int depth) {
      if(depth>32||position>=text.Length)Invalid();char first=text[position];
      if(first=='"'){String();return;}
      if(first=='{'||first=='[') {
        position++;bool obj=first=='{';char end=obj?'}':']';int count=0;string previous=null;
        if(position<text.Length&&text[position]==end){position++;return;}
        for(;;) {
          if(++count>4096)Invalid();
          if(obj){string key=String();if(previous!=null&&System.String.CompareOrdinal(previous,key)>=0)Invalid();previous=key;if(Next()!=':')Invalid();}
          Value(depth+1);char separator=Next();if(separator==end)return;if(separator!=',')Invalid();
        }
      }
      if(first>='0'&&first<='9') {
        long value=0;int start=position;
        while(position<text.Length&&text[position]>='0'&&text[position]<='9') {
          int digit=text[position++]-'0';if(value>(9007199254740991L-digit)/10)Invalid();value=value*10+digit;
        }
        if(position-start>1&&first=='0')Invalid();return;
      }
      foreach(string literal in new[]{"true","false","null"}) {
        if(position+literal.Length<=text.Length&&System.String.CompareOrdinal(text,position,literal,0,literal.Length)==0){position+=literal.Length;return;}
      }
      Invalid();
    }
    public void Check(){if(text.Length==0||text[0]!='{')Invalid();Value(0);if(position!=text.Length)Invalid();}
  }
  public static void CheckCanonicalObject(byte[] bytes){new PrivateJsonFraming(bytes).Check();}
  public static byte[] ReadBoundedFile(string file,int maximum) {
    using(var input=new System.IO.FileStream(file,System.IO.FileMode.Open,System.IO.FileAccess.Read,System.IO.FileShare.Read)) {
      if(input.Length<1||input.Length>maximum)throw new InvalidOperationException();
      byte[] bytes=new byte[(int)input.Length];int used=0;
      while(used<bytes.Length){int count=input.Read(bytes,used,bytes.Length-used);if(count<1)throw new InvalidOperationException();used+=count;}
      if(input.ReadByte()!=-1)throw new InvalidOperationException();return bytes;
    }
  }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern IntPtr LoadLibraryExW(string file, IntPtr reserved, uint flags);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool FreeLibrary(IntPtr module);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern IntPtr FindResourceExW(IntPtr module, IntPtr type, IntPtr name, ushort language);
  [DllImport("kernel32.dll", SetLastError=true)] static extern uint SizeofResource(IntPtr module, IntPtr resource);
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
  [DllImport("kernel32.dll")] static extern IntPtr LockResource(IntPtr resource);
  delegate bool Languages(IntPtr module, IntPtr type, IntPtr name, ushort language, IntPtr parameter);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool EnumResourceLanguagesW(IntPtr module, IntPtr type, IntPtr name, Languages callback, IntPtr parameter);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern IntPtr BeginUpdateResourceW(string file, bool deleteExisting);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool UpdateResourceW(IntPtr update, IntPtr type, IntPtr name, ushort language, byte[] bytes, uint size);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool EndUpdateResourceW(IntPtr update, bool discard);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern int CompareStringOrdinal(string a, int alen, string b, int blen, bool ignoreCase);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern uint GetFileAttributesW(string file);
  static readonly IntPtr Type = (IntPtr)10;
  public static byte[] Read(string file, int id, bool absentAllowed) {
    if(id<34101||id>34109)throw new InvalidOperationException();
    IntPtr name=(IntPtr)id;
    IntPtr module=LoadLibraryExW(file,IntPtr.Zero,0x22);
    if(module==IntPtr.Zero) throw new Win32Exception();
    try {
      int count=0; bool neutral=true;
      Languages callback=(m,t,n,l,p)=>{count++;neutral=neutral && l==0;return true;};
      bool found=EnumResourceLanguagesW(module,Type,name,callback,IntPtr.Zero);
      int error=Marshal.GetLastWin32Error();
      GC.KeepAlive(callback);
      if(!found && count==0 && absentAllowed && (error==1812||error==1813||error==1814||error==1815))return null;
      if(!found||count!=1||!neutral)throw new InvalidOperationException();
      IntPtr resource=FindResourceExW(module,Type,name,0);
      uint size=SizeofResource(module,resource);
      if(resource==IntPtr.Zero||size==0||size>1048576)throw new InvalidOperationException();
      IntPtr bytes=LockResource(LoadResource(module,resource));
      if(bytes==IntPtr.Zero)throw new Win32Exception();
      byte[] result=new byte[size];Marshal.Copy(bytes,result,0,(int)size);return result;
    } finally { if(!FreeLibrary(module))throw new Win32Exception(); }
  }
  public static void CheckResources(string file,byte[][] resources) {
    if(resources.Length<2||resources.Length>9)throw new InvalidOperationException();
    for(int i=0;i<9;i++) {
      byte[] actual=Read(file,34101+i,i>=resources.Length);
      if(i>=resources.Length){if(actual!=null)throw new InvalidOperationException();continue;}
      byte[] expected=resources[i];
      if(actual.Length!=expected.Length)throw new InvalidOperationException();
      for(int j=0;j<actual.Length;j++)if(actual[j]!=expected[j])throw new InvalidOperationException();
    }
  }
  public static void Embed(string file,byte[][] resources) {
    if(resources.Length<2||resources.Length>9)throw new InvalidOperationException();
    for(int i=0;i<9;i++)if(Read(file,34101+i,true)!=null)throw new InvalidOperationException();
    foreach(byte[] bytes in resources)if(bytes.Length==0||bytes.Length>1048576)throw new InvalidOperationException();
    IntPtr update=BeginUpdateResourceW(file,false);
    if(update==IntPtr.Zero)throw new Win32Exception();
    bool ended=false;
    try {
      for(int i=0;i<resources.Length;i++) {
        byte[] bytes=resources[i];
        if(!UpdateResourceW(update,Type,(IntPtr)(34101+i),0,bytes,(uint)bytes.Length))throw new Win32Exception();
      }
      ended=true;
      if(!EndUpdateResourceW(update,false))throw new Win32Exception();
    } finally { if(!ended)EndUpdateResourceW(update,true); }
    CheckResources(file,resources);
  }
  public static bool SameName(string a,string b) {
    int result=CompareStringOrdinal(a,a.Length,b,b.Length,true);
    if(result==0)throw new Win32Exception();return result==2;
  }
  public static int CompareUtf8(string a,string b) {
    var encoding=new System.Text.UTF8Encoding(false,true);
    byte[] left=encoding.GetBytes(a),right=encoding.GetBytes(b);
    int length=Math.Min(left.Length,right.Length);
    for(int i=0;i<length;i++)if(left[i]!=right[i])return left[i]<right[i]?-1:1;
    return left.Length.CompareTo(right.Length);
  }
  static int CompareNames(string a,string b) {
    int result=CompareStringOrdinal(a,a.Length,b,b.Length,true);
    if(result==0)throw new Win32Exception();return result-2;
  }
  public static void CheckInventory(string[] names,string[] sources) {
    if(names.Length!=sources.Length||names.Length<1||names.Length>32768)throw new InvalidOperationException();
    foreach(string file in sources)CheckFile(file);
    CheckNames(names);
  }
  public static void CheckNames(string[] names) {
    if(names.Length<1||names.Length>32768)throw new InvalidOperationException();
    Array.Sort(names,CompareNames);
    var comparer=System.Collections.Generic.Comparer<string>.Create(CompareNames);
    for(int i=1;i<names.Length;i++) {
      string previous=names[i-1], current=names[i];
      if(SameName(previous,current))throw new InvalidOperationException();
    }
    // A punctuation entry can sort between a file and its descendants (a,
    // a-, a/file). Search EVERY actual ancestor with the same native comparator,
    // rather than assuming the offending file is the adjacent sorted entry.
    foreach(string name in names) {
      int slash=name.IndexOf('/');
      while(slash>=0) {
        if(Array.BinarySearch(names,name.Substring(0,slash),comparer)>=0)throw new InvalidOperationException();
        slash=name.IndexOf('/',slash+1);
      }
    }
  }
  public static void CheckFile(string file) {
    string current=System.IO.Path.GetFullPath(file);bool leaf=true;
    for(;;) {
      uint flags=GetFileAttributesW(current);
      if(flags==0xffffffff||(flags&0x400)!=0||(leaf&&(flags&0x10)!=0))throw new InvalidOperationException();
      string parent=System.IO.Path.GetDirectoryName(current);
      if(String.IsNullOrEmpty(parent)||parent==current)break;
      current=parent;leaf=false;
    }
  }
}
'@
  function Assert-Object($Value,[string[]]$Keys) {
    if ($null -eq $Value -or $Value -isnot [Management.Automation.PSCustomObject]) { throw 'object' }
    $names = @($Value.PSObject.Properties.Name)
    if ($names.Count -ne $Keys.Count) { throw 'object' }
    foreach ($key in $Keys) { if ($names -cnotcontains $key) { throw 'object' } }
  }
  function Assert-Integer($Value,[long]$Minimum,[long]$Maximum) {
    if (($Value -isnot [int] -and $Value -isnot [long] -and $Value -isnot [double] -and $Value -isnot [decimal]) -or
        $Value -lt $Minimum -or $Value -gt $Maximum -or [double]::IsNaN([double]$Value) -or
        [double]::IsInfinity([double]$Value) -or [Math]::Floor([double]$Value) -ne [double]$Value) { throw 'integer' }
  }
  function Assert-Array($Value,[int]$Minimum,[int]$Maximum) {
    if ($Value -isnot [Array] -or $Value.Count -lt $Minimum -or $Value.Count -gt $Maximum) { throw 'array' }
  }
  function Assert-Hash($Value) {
    if ($Value -isnot [string] -or $Value -cnotmatch '^[0-9a-f]{64}$') { throw 'hash' }
  }
  function Byte-Hash([byte[]]$Bytes) {
    $hash = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($hash.ComputeHash($Bytes))).Replace('-','').ToLowerInvariant() } finally { $hash.Dispose() }
  }
  function Read-BoundedJson([string]$File) {
    [PinvouHostBuildResource]::CheckFile($File)
    $bytes = [PinvouHostBuildResource]::ReadBoundedFile($File,1048576)
    [PinvouHostBuildResource]::CheckCanonicalObject($bytes)
    $text = ([Text.UTF8Encoding]::new($false,$true)).GetString($bytes)
    if ($text.Length -eq 0 -or [int]$text[0] -eq 65279) { throw 'encoding' }
    # Every input is a separately bounded builder-created canonical object.
    # Never join the shard/control text into one oversized JSON parser input.
    return @{ Bytes=$bytes; Value=($text | ConvertFrom-Json) }
  }
  function Assert-Name($Name) {
    if ($Name -isnot [string] -or !$Name -or [Text.Encoding]::UTF8.GetByteCount($Name) -gt 1024 -or
        $Name -match '[\\:\x00-\x1f\x7f]' -or $Name.StartsWith('/')) { throw 'name' }
    $parts = $Name.Split('/')
    if ($parts.Count -gt 32) { throw 'name' }
    foreach ($part in $parts) {
      if (!$part -or $part -eq '.' -or $part -eq '..' -or $part -match '[. ]$|[<>"|?*]' -or
          $part -match '^(con|prn|aux|nul|conin\$|conout\$|com[1-9\u00b9\u00b2\u00b3]|lpt[1-9\u00b9\u00b2\u00b3])(?:\.|$)') { throw 'name' }
    }
  }
  function Read-ManifestResources([string]$File) {
    $root = Read-BoundedJson $File
    $manifestValue = $root.Value
    Assert-Object $manifestValue @('hostManifestVersion','profileKind','profileId','product','component','targetKey','buildId','seedSha256','originalRoot','layoutId','helper','launcher','app','appInventory')
    Assert-Integer $manifestValue.hostManifestVersion 2 2
    if ($manifestValue.hostManifestVersion -cne 2) { throw 'version' }
    if ($manifestValue.profileKind -cnotin @('lab','production') -or $manifestValue.profileId -isnot [string] -or
        $manifestValue.profileId -cnotmatch '^[a-z0-9][a-z0-9-]{0,47}$' -or
        ($manifestValue.profileKind -ceq 'production' -and $manifestValue.profileId.Contains('-fault-')) -or
        $manifestValue.product -cne 'pinvou' -or $manifestValue.component -isnot [string] -or
        $manifestValue.component -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$' -or
        $manifestValue.targetKey -cne 'windows-x86_64-nsis-perMachine' -or $manifestValue.layoutId -cne 'windows-host-v1') { throw 'scope' }
    Assert-Hash $manifestValue.buildId
    Assert-Hash $manifestValue.seedSha256
    Assert-Object $manifestValue.originalRoot @('size','sha256')
    Assert-Integer $manifestValue.originalRoot.size 1 1048576
    Assert-Hash $manifestValue.originalRoot.sha256
    foreach ($role in @('helper','launcher','app')) {
      $imageValue=$manifestValue.$role
      Assert-Object $imageValue @('buildId','version','size','sha256','layoutId')
      Assert-Integer $imageValue.size 1 9007199254740991
      Assert-Hash $imageValue.sha256
      if ($imageValue.buildId -cne $manifestValue.buildId -or $imageValue.layoutId -cne $manifestValue.layoutId -or
          $imageValue.version -isnot [string] -or $imageValue.version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
          $imageValue.version -cne $manifestValue.app.version) { throw 'image' }
    }
    $inventoryValue = $manifestValue.appInventory
    Assert-Object $inventoryValue @('hostInventoryVersion','fileCount','totalSize','shards')
    Assert-Integer $inventoryValue.hostInventoryVersion 1 1
    if ($inventoryValue.hostInventoryVersion -cne 1) { throw 'version' }
    Assert-Integer $inventoryValue.fileCount 1 32768
    Assert-Integer $inventoryValue.totalSize 1 9007199254740991
    Assert-Array $inventoryValue.shards 1 8
    $resources = [Collections.Generic.List[byte[]]]::new()
    $resources.Add($root.Bytes)
    $names = [Collections.Generic.List[string]]::new()
    [long]$fileCount=0; [long]$totalSize=0; $previous=$null; [int]$mainCount=0
    for ($index=0; $index -lt $inventoryValue.shards.Count; $index++) {
      $reference=$inventoryValue.shards[$index]
      Assert-Object $reference @('index','resourceId','size','sha256','fileCount')
      Assert-Integer $reference.index $index $index
      Assert-Integer $reference.resourceId (34102+$index) (34102+$index)
      Assert-Integer $reference.size 1 1048576
      Assert-Integer $reference.fileCount 1 4096
      if ($reference.sha256 -isnot [string] -or $reference.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'hash' }
      $shard = Read-BoundedJson (Join-Path ([IO.Path]::GetDirectoryName($File)) "installation-inventory-$index.json")
      if ($shard.Bytes.Length -ne $reference.size -or (Byte-Hash $shard.Bytes) -cne $reference.sha256) { throw 'hash' }
      $value=$shard.Value
      Assert-Object $value @('hostInventoryShardVersion','profileId','buildId','seedSha256','index','files')
      Assert-Integer $value.hostInventoryShardVersion 1 1
      if ($value.hostInventoryShardVersion -cne 1 -or $value.profileId -cne $manifestValue.profileId -or
          $value.buildId -cne $manifestValue.buildId -or $value.seedSha256 -cne $manifestValue.seedSha256) { throw 'scope' }
      Assert-Integer $value.index $index $index
      Assert-Array $value.files 1 4096
      if ($value.files.Count -ne $reference.fileCount) { throw 'count' }
      foreach ($entry in $value.files) {
        Assert-Object $entry @('relativeName','size','sha256')
        Assert-Name $entry.relativeName
        Assert-Integer $entry.size 0 9007199254740991
        if ($entry.sha256 -isnot [string] -or $entry.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'hash' }
        if ($null -ne $previous -and [PinvouHostBuildResource]::CompareUtf8($previous,$entry.relativeName) -ge 0) { throw 'order' }
        $previous=$entry.relativeName; $names.Add($entry.relativeName)
        if ($totalSize -gt (9007199254740991-$entry.size)) { throw 'sum' }
        $totalSize += $entry.size; $fileCount++
        if ($entry.relativeName -ceq 'pinvou3-tauri.exe') {
          $mainCount++
          Assert-Integer $entry.size 1 9007199254740991
          if ($entry.size -ne $manifestValue.app.size -or $entry.sha256 -cne $manifestValue.app.sha256) { throw 'app' }
        }
      }
      $resources.Add($shard.Bytes)
    }
    if ($fileCount -ne $inventoryValue.fileCount -or $totalSize -ne $inventoryValue.totalSize -or $mainCount -ne 1) { throw 'totals' }
    [PinvouHostBuildResource]::CheckNames($names.ToArray())
    return ,$resources.ToArray()
  }
  if ($Mode -eq 'Inventory') {
    $control=(Read-BoundedJson $Inventory).Value
    Assert-Object $control @('hostNativeInventoryVersion','fileCount','blocks')
    Assert-Integer $control.hostNativeInventoryVersion 1 1
    if ($control.hostNativeInventoryVersion -cne 1) { throw 'version' }
    Assert-Integer $control.fileCount 1 32768
    Assert-Array $control.blocks 1 16
    $names=[Collections.Generic.List[string]]::new(); $sources=[Collections.Generic.List[string]]::new()
    for ($index=0; $index -lt $control.blocks.Count; $index++) {
      $reference=$control.blocks[$index]
      Assert-Object $reference @('index','fileName','size','sha256','fileCount')
      Assert-Integer $reference.index $index $index
      Assert-Integer $reference.size 1 1048576
      Assert-Integer $reference.fileCount 1 4096
      $fileName='native-inventory-{0:D3}.json' -f $index
      if ($reference.fileName -cne $fileName -or $reference.sha256 -isnot [string] -or $reference.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'reference' }
      $block=Read-BoundedJson (Join-Path ([IO.Path]::GetDirectoryName($Inventory)) $fileName)
      if ($block.Bytes.Length -ne $reference.size -or (Byte-Hash $block.Bytes) -cne $reference.sha256) { throw 'hash' }
      $value=$block.Value
      Assert-Object $value @('hostNativeInventoryBlockVersion','index','entries')
      Assert-Integer $value.hostNativeInventoryBlockVersion 1 1
      if ($value.hostNativeInventoryBlockVersion -cne 1) { throw 'version' }
      Assert-Integer $value.index $index $index
      Assert-Array $value.entries 1 4096
      if ($value.entries.Count -ne $reference.fileCount -or ($names.Count+$value.entries.Count) -gt $control.fileCount) { throw 'count' }
      foreach ($entry in $value.entries) {
        Assert-Object $entry @('relativeName','source')
        Assert-Name $entry.relativeName
        if ($entry.source -isnot [string] -or [Text.Encoding]::UTF8.GetByteCount($entry.source) -gt 3072 -or
            $entry.source -match '[\x00-\x1f\x7f]' -or ![IO.Path]::IsPathRooted($entry.source)) { throw 'source' }
        $names.Add($entry.relativeName); $sources.Add($entry.source)
      }
    }
    if ($names.Count -ne $control.fileCount) { throw 'count' }
    # The aggregate is typed internal memory only. Every parser input above
    # remains a closed block within its original 1 MiB / 4096-entry bounds.
    [PinvouHostBuildResource]::CheckInventory($names.ToArray(),$sources.ToArray())
    [Console]::Out.WriteLine('HOST_BUILD_INVENTORY_CHECKED')
  } elseif ($Mode -eq 'VerifyImage') {
    [PinvouHostBuildResource]::CheckFile($Image)
    # The caller can inherit a PS7 module path while this adapter runs in
    # Windows PowerShell 5. Load this interpreter's native verifier explicitly.
    Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
    $signature = Get-AuthenticodeSignature -LiteralPath $Image
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode' -or $null -eq $signature.SignerCertificate) { throw 'signature' }
    $hash = [Security.Cryptography.SHA256]::Create()
    try { $actual = ([BitConverter]::ToString($hash.ComputeHash($signature.SignerCertificate.RawData))).Replace('-','').ToLowerInvariant() } finally { $hash.Dispose() }
    if ($ExpectedSigner -cnotmatch '^[0-9a-f]{64}$' -or $actual -cne $ExpectedSigner) { throw 'signer' }
    [Console]::Out.WriteLine('HOST_BUILD_IMAGE_CHECKED')
  } else {
    $resources=Read-ManifestResources $Manifest
    if ($Mode -eq 'Embed') { [PinvouHostBuildResource]::Embed($Image,$resources) }
    [PinvouHostBuildResource]::CheckResources($Image,$resources)
    [Console]::Out.WriteLine('HOST_BUILD_RESOURCE_CHECKED')
  }
  exit 0
} catch {
  # Provider/native/path/credential details are never printed by the build bridge.
  [Console]::Error.WriteLine('HOST_BUILD_NATIVE_CHECK_FAILED')
  exit 1
}
