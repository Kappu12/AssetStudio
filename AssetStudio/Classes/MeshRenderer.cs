using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;

namespace AssetStudio
{
    public sealed class MeshRenderer : Renderer
    {
        public PPtr<Mesh> m_AdditionalVertexStreams;
        public PPtr<Mesh> m_EnlightenVertexStream;
        public MeshRenderer(ObjectReader reader) : base(reader)
        {
            m_AdditionalVertexStreams = new PPtr<Mesh>(reader);
            if (version[0] >= 6000)
            {
                m_EnlightenVertexStream = new PPtr<Mesh>(reader);
            }
        }
    }
}
