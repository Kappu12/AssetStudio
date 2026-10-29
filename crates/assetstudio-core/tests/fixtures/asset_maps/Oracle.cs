using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
using MessagePack;
[MessagePackObject] public sealed class AssetMap { [Key(0)] public int GameType {get;set;} [Key(1)] public List<AssetEntry> AssetEntries {get;set;} = new(); }
[MessagePackObject] public sealed class AssetEntry { [Key(0)] public string Name {get;set;} = ""; [Key(1)] public string Container {get;set;} = ""; [Key(2)] public string Source {get;set;} = ""; [Key(3)] public long PathID {get;set;} [Key(4)] public int Type {get;set;} }
public static class Program {
 static AssetMap Sample(int count,bool small=false) {var map=new AssetMap();for(int i=0;i<count;i++)map.AssetEntries.Add(new AssetEntry {Name=small?"a":"Icon_Åäö_"+i+new string('x',80),Container=small?"":"Assets/UI/"+i+".txt",Source=small?"b.assets":"fixtures/source_"+(i%3)+".bundle",PathID=small?7:(i==0?long.MinValue:(i==count-1?long.MaxValue:i)),Type=i%2==0?49:-1});return map;}
 static void Save(string directory,string name,AssetMap map,MessagePackCompression compression) {var options=MessagePackSerializerOptions.Standard.WithCompression(compression);File.WriteAllBytes(Path.Combine(directory,name+".map"),MessagePackSerializer.Serialize(map,options));File.WriteAllText(Path.Combine(directory,name+".json"),JsonSerializer.Serialize(map));Console.WriteLine(name+":"+map.AssetEntries.Count);}
 public static int Main(string[] args) {
  if(args.Length==2&&args[0]=="--decode") {var map=MessagePackSerializer.Deserialize<AssetMap>(File.ReadAllBytes(args[1]),MessagePackSerializerOptions.Standard.WithCompression(MessagePackCompression.Lz4BlockArray));Console.WriteLine(JsonSerializer.Serialize(map));return 0;}
  Directory.CreateDirectory(args[0]);Save(args[0],"csharp_plain",Sample(1,true),MessagePackCompression.Lz4BlockArray);Save(args[0],"csharp_lz4_array",Sample(2),MessagePackCompression.Lz4BlockArray);Save(args[0],"csharp_lz4_multiblock",Sample(3000),MessagePackCompression.Lz4BlockArray);Save(args[0],"csharp_lz4_single",Sample(2),MessagePackCompression.Lz4Block);return 0;
 }
}