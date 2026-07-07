using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;
using SixLabors.ImageSharp.Processing;
using System.Buffers;
using System.IO;

namespace AssetStudio
{
    public static class Texture2DExtensions
    {
        private static Configuration _configuration;

        static Texture2DExtensions()
        {
            _configuration = Configuration.Default.Clone();
            _configuration.PreferContiguousImageBuffers = true;
        }
        public static Image<Bgra32> ConvertToImage(this Texture2D m_Texture2D, bool flip)
        {
            var outputSize = GetOutputBufferSize(m_Texture2D);
            if (outputSize <= 0)
            {
                return null;
            }
            var converter = new Texture2DConverter(m_Texture2D);
            var buff = ArrayPool<byte>.Shared.Rent(outputSize);
            try
            {
                if (converter.DecodeTexture2D(buff))
                {
                    var image = Image.LoadPixelData<Bgra32>(_configuration, buff, m_Texture2D.m_Width, m_Texture2D.m_Height);
                    if (flip)
                    {
                        image.Mutate(x => x.Flip(FlipMode.Vertical));
                    }
                    return image;
                }
                return null;
            }
            finally
            {
                ArrayPool<byte>.Shared.Return(buff, true);
            }
        }

        private static int GetOutputBufferSize(Texture2D texture)
        {
            if (texture.m_Width <= 0 || texture.m_Height <= 0)
            {
                return 0;
            }

            var size = (long)texture.m_Width * texture.m_Height * 4;
            return size > int.MaxValue ? 0 : (int)size;
        }

        public static MemoryStream ConvertToStream(this Texture2D m_Texture2D, ImageFormat imageFormat, bool flip)
        {
            var image = ConvertToImage(m_Texture2D, flip);
            if (image != null)
            {
                using (image)
                {
                    return image.ConvertToStream(imageFormat);
                }
            }
            return null;
        }
    }
}
