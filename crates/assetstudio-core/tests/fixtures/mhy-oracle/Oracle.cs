using System;
using System.IO;
using AssetStudio;
namespace AssetStudio {
 public static class Logger {public static void Verbose(string message) {}}
 public static class Compatibility {public static bool IsNullOrEmpty(this byte[] bytes)=>bytes==null||bytes.Length==0;}
 public sealed class Mr0k {public byte[] ExpansionKey,SBox,InitVector,BlockKey,PostKey;public Mr0k(byte[] expansion,byte[] block,byte[] iv=null,byte[] post=null){ExpansionKey=expansion;BlockKey=block;InitVector=iv??Array.Empty<byte>();PostKey=post??Array.Empty<byte>();SBox=Array.Empty<byte>();}}
}
public static class Program {
 public static int Main(string[] args){
  Directory.CreateDirectory(args[0]);
  foreach(var seed in new uint[]{0,1,4096,uint.MaxValue}) {XORShift128.InitSeed(seed);Console.WriteLine($"rng {seed:X8}: {XORShift128.NextDecryptUInt():X8} {XORShift128.NextDecryptUInt():X8} {XORShift128.NextDecryptUInt():X8} {unchecked((ulong)XORShift128.NextDecryptLong()):X16}");}
  foreach(var item in new (string,int)[]{("BH3PrePre",148),("BH3PrePre",276),("BH3PrePre",4096),("BH3PrePre",10000),("TOT",3092),("TOT",4096),("TOT",10000)}){
    var data=new byte[item.Item2];for(int i=0;i<data.Length;i++)data[i]=unchecked((byte)(i*37+11));new byte[]{0x6d,0x72,0x30,0x6b}.CopyTo(data,0);
    var config=item.Item1=="TOT"?new Mr0k(Crypto.Mr0kExpansionKey,Crypto.Mr0kBlockKey,Crypto.Mr0kInitVector,Crypto.ToTKey):new Mr0k(Crypto.PackExpansionKey,Crypto.PackBlockKey);
    var output=Mr0kUtils.Decrypt(data,config).ToArray();var name=$"{item.Item1}-{item.Item2}.bin";File.WriteAllBytes(Path.Combine(args[0],name),output);Console.WriteLine(name+" "+Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(output)));
  }
  foreach(var profile in new[]{"BH3PrePre","TOT"}) {
    var data=new byte[4096];for(int i=0;i<data.Length;i++)data[i]=unchecked((byte)(i*37+11));new byte[]{0x6d,0x72,0x30,0x6b}.CopyTo(data,0);
    for(int i=0;i<17;i++){byte literal=i==0?(byte)0xf0:(i==16?(byte)219:(byte)255);data[20+i]=(byte)(literal^(profile=="TOT"?Crypto.ToTKey[i%Crypto.ToTKey.Length]:0));}
    File.WriteAllBytes(Path.Combine(args[0],profile+"-literal.mr0k"),data);
    var config=profile=="TOT"?new Mr0k(Crypto.Mr0kExpansionKey,Crypto.Mr0kBlockKey,Crypto.Mr0kInitVector,Crypto.ToTKey):new Mr0k(Crypto.PackExpansionKey,Crypto.PackBlockKey);
    var output=Mr0kUtils.Decrypt(data,config).ToArray();
    File.WriteAllBytes(Path.Combine(args[0],profile+"-literal-payload.bin"),output.AsSpan(17).ToArray());
  }
  var lit=new System.Collections.Generic.List<byte>{0xf8};lit.AddRange(System.Text.Encoding.ASCII.GetBytes("ABCDABCD"));lit.AddRange(new byte[]{4,0,3,0x0f,5});lit.AddRange(System.Text.Encoding.ASCII.GetBytes("01234567890123456789"));var plain=new byte[50];int wrote=LZ4Lit.Instance.Decompress(lit.ToArray(),plain);if(wrote!=plain.Length)throw new Exception("LZ4Lit oracle size mismatch");File.WriteAllBytes(Path.Combine(args[0],"LZ4Lit-reference.lit"),lit.ToArray());File.WriteAllBytes(Path.Combine(args[0],"LZ4Lit-reference.bin"),plain);
  var lndplain=new byte[256];for(int i=0;i<lndplain.Length;i++)lndplain[i]=unchecked((byte)(i*19+71));byte[] lndheader={0x55,0x6e,0x69,0x74,0x79,0x46,0x53,0,0,0,0,7,0x35,0x2e,0x78,0x2e};lndheader.CopyTo(lndplain,0);byte[] lndvector={0x35,0x6b,0x05,0};var lndencoded=(byte[])lndplain.Clone();for(int i=0;i<lndencoded.Length;i++)lndencoded[i]^=(byte)(unchecked((byte)((i%16)*17+3))^lndvector[(i/16)%4]);File.WriteAllBytes(Path.Combine(args[0],"LnD-reference.encrypted"),lndencoded);var lndreader=ImportHelper.DecryptLoveAndDeepspace(new FileReader("synthetic.bundle",new MemoryStream(lndencoded)));File.WriteAllBytes(Path.Combine(args[0],"LnD-reference.bin"),lndreader.ReadBytes((int)lndreader.Remaining));
  return 0;
 }
}
