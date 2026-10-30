using System;
using System.IO;
using System.Collections.Generic;
using System.Security.Cryptography;
using System.Text;
using AssetStudio;
namespace AssetStudio { public static class Logger { public static void Verbose(string text) {} } }
public static class Program {
 static string destination;
 static void Save(string name, byte[] data) { File.WriteAllBytes(Path.Combine(destination,name),data); Console.WriteLine(name+" "+data.Length+" "+Convert.ToHexString(SHA256.HashData(data))); }
 static byte[] Pattern(int length) { var data=new byte[length]; for(int i=0;i<length;i++) data[i]=unchecked((byte)(i*37+11)); return data; }
 static void Compressed(string name, byte[] input, int expanded, bool inverted) {
   Save(name+".encrypted",input); var decoded=(byte[])input.Clone(); FairGuardUtils.Decrypt(decoded); Save(name+".compressed",decoded); var payload=new byte[expanded]; int count=(inverted?LZ4Inv.Instance:LZ4.Instance).Decompress(decoded,payload); if(count!=expanded) throw new Exception("Oracle decompressed size mismatch"); Save(name+".bin",payload);
 }
 public static int Main(string[] args) {
  destination=args[0]; Directory.CreateDirectory(destination);
  foreach(int length in new[]{0,1,31,32,33,80,159,160,161,255,1279,1280,1281,4096}) {
   var bytes=Pattern(length); FairGuardUtils.Decrypt(bytes); Save($"fairguard-{length}.bin",bytes);
  }
  foreach(bool inverted in new[]{false,true}) {
   int length=4096, literal=0; var header=new List<byte>();
   for(int candidate=15;candidate<length;candidate++) if(candidate+2+(candidate-15)/255==length) {literal=candidate;break;}
   if(literal==0) throw new Exception("Literal length unavailable");
   header.Add(inverted?(byte)0x0f:(byte)0xf0); int extra=literal-15; while(extra>=255) {header.Add(255);extra-=255;} header.Add((byte)extra);
   var input=Pattern(length); for(int i=0;i<header.Count;i++) input[i]=(byte)(header[i]^0xa6);
   Compressed(inverted?"fairguard-literal-inv":"fairguard-literal-lz4",input,literal,inverted);
  }
  var complex=new List<byte>{0xf8}; complex.AddRange(Encoding.ASCII.GetBytes("ABCDABCD")); complex.AddRange(new byte[]{0,4,3,0x0f,5}); complex.AddRange(Encoding.ASCII.GetBytes("01234567890123456789"));
  Save("lz4inv-reference.inv",complex.ToArray()); var known=new byte[50]; if(LZ4Inv.Instance.Decompress(complex.ToArray(),known)!=known.Length) throw new Exception("LZ4Inv count mismatch"); Save("lz4inv-reference.bin",known);
  var encrypted=complex.ToArray(); for(int i=0;i<32;i++) encrypted[i]^=0xa6; Compressed("fairguard-overlap-inv",encrypted,50,true);
  return 0;
 }
}
