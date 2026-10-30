using System;using System.IO;namespace AssetStudio{public static class ImportHelper{
        public static FileReader DecryptLoveAndDeepspace(FileReader reader)
        {
            Logger.Verbose($"Attempting to decrypt file {reader.FileName} with Love And Deepspace encryption");

            var vector = new byte[] { 0x35, 0x6B, 0x05, 0x00 };
            var originalHeader = new byte[] { 0x55, 0x6E, 0x69, 0x74, 0x79, 0x46, 0x53, 0x00, 0x00, 0x00, 0x00, 0x07, 0x35, 0x2E, 0x78, 0x2E };

            var seed = reader.ReadBytes(0x10);
            for (int i = 0; i < seed.Length; i++)
            {
                var b = (byte)(seed[i] ^ originalHeader[i] ^ vector[0]);
                seed[i] = b != originalHeader[i] ? b : originalHeader[i];
            }

            var key = new byte[0x40];
            for (int i = 0; i < vector.Length; i++)
            {
                for (int j = 0; j < seed.Length; j++)
                {
                    var offset = i * 0x10;
                    key[offset + j] = (byte)(seed[j] ^ vector[i]);
                }
            }

            reader.Position = 0;
            var data = reader.ReadBytes((int)reader.Remaining);
            for (int i = 0; i < data.Length; i++)
            {
                data[i] ^= key[i % key.Length];
            }

            Logger.Verbose("Decrypted Love And Deepspace file successfully !!");

            MemoryStream ms = new();
            ms.Write(data);
            ms.Position = 0;
            return new FileReader(reader.FullPath, ms);
        }

}
public sealed class FileReader{public string FullPath;public string FileName=>FullPath;private Stream stream;public FileReader(string path,Stream data){FullPath=path;stream=data;}public long Remaining=>stream.Length-stream.Position;public long Position{get=>stream.Position;set=>stream.Position=value;}public byte[] ReadBytes(int count){byte[] data=new byte[count];int read=stream.Read(data);Array.Resize(ref data,read);return data;}}}
