// SPDX-License-Identifier: MIT
// Development-only edge extension of oracle.cpp; see EDGE_CASES.md.
// Never compiled or linked by Cargo.
// Build against nfrechette/acl v2.1.0 (414689d5cff4286a7898487a46dc5e48005d38da)
// and its pinned RTM submodule; run with the fixture directory as argv[1].
#include <acl/compression/compress.h>
#include <acl/compression/track_array.h>
#include <acl/compression/output_stats.h>
#include <acl/core/ansi_allocator.h>
#include <acl/core/track_writer.h>
#include <acl/decompression/decompress.h>
#include <acl/decompression/decompression_settings.h>
#include <rtm/quatf.h>
#include <rtm/vector4f.h>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

struct writer : acl::track_writer {
    std::vector<float> values;
    uint32_t width;
    writer(uint32_t tracks, uint32_t n) : values(tracks*n, 0), width(n) {}
    void RTM_SIMD_CALL write_float1(uint32_t i, rtm::scalarf_arg0 v) { values[i*width] = rtm::scalar_cast(v); }
    void vector(uint32_t i, rtm::vector4f_arg0 v, uint32_t n, uint32_t off=0) {
        float f[4]; rtm::vector_store(v, f);
        for (uint32_t c=0; c<n; ++c) values[i*width+off+c] = f[c];
    }
    void RTM_SIMD_CALL write_float2(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,2); }
    void RTM_SIMD_CALL write_float3(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,3); }
    void RTM_SIMD_CALL write_float4(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,4); }
    void RTM_SIMD_CALL write_vector4(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,4); }
    void RTM_SIMD_CALL write_rotation(uint32_t i, rtm::quatf_arg0 q) { vector(i,rtm::quat_to_vector(q),4); }
    void RTM_SIMD_CALL write_translation(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,3,4); }
    void RTM_SIMD_CALL write_scale(uint32_t i, rtm::vector4f_arg0 v) { vector(i,v,3,7); }
};

struct oracle_settings : acl::debug_transform_decompression_settings {
    static constexpr bool is_track_type_supported(acl::track_type8) { return true; }
};

void save(acl::iallocator& allocator, const acl::track_array& raw,
          acl::compression_settings settings, const std::string& path, uint32_t width) {
    acl::compressed_tracks* tracks = nullptr;
    acl::output_stats stats;
    auto result = acl::compress_track_list(allocator,raw,settings,tracks,stats);
    if (result.any()) { std::fprintf(stderr,"compression: %s\n",result.c_str()); std::exit(2); }
    FILE* f = std::fopen((path+".acl").c_str(),"wb");
    if (!f) std::exit(3);
    std::fwrite(tracks,1,tracks->get_size(),f); std::fclose(f);
    acl::decompression_context<oracle_settings> ctx;
    if (!ctx.initialize(*tracks)) std::exit(4);
    f = std::fopen((path+".csv").c_str(),"w");
    if (!f) std::exit(3);
    std::fprintf(f,"%u,%u,%u,%.9g\n",tracks->get_num_tracks(),tracks->get_num_samples_per_track(),width,tracks->get_sample_rate());
    for (uint32_t frame=0; frame<tracks->get_num_samples_per_track(); ++frame) {
        writer out(tracks->get_num_tracks(),width);
        ctx.seek(float(frame)/tracks->get_sample_rate(),acl::sample_rounding_policy::nearest);
        ctx.decompress_tracks(out);
        for (size_t c=0; c<out.values.size(); ++c)
            std::fprintf(f,c ? ",%.9g" : "%.9g",out.values[c]);
        std::fputc('\n',f);
    }
    std::fclose(f);
    allocator.deallocate(tracks,tracks->get_size());
}

template<acl::track_type8 Type> void scalars(acl::ansi_allocator& a,const std::string& dir,const char* name,uint32_t width) {
    constexpr uint32_t count=5, frames=37;
    using track=acl::track_typed<Type>;
    using sample=typename track::sample_type;
    acl::track_array_typed<Type> raw(a,count);
    for (uint32_t i=0; i<count; ++i) {
        std::vector<sample> data(frames);
        for (uint32_t f=0; f<frames; ++f) {
            float values[4]={};
            for(uint32_t c=0;c<width;++c) values[c]=i==0 ? 2.5F+c :
                i==1 ? std::sin(float(f)*(0.1F+0.03F*c))*10.0F+c :
                i==2 ? float(f)*0.125F*(c+1) :
                i==3 ? ((f%3)==0 ? 100000.125F : -0.03125F)*(c+1) :
                       std::cos(float(f)*0.7F+c)*0.01F;
            std::memcpy(&data[f],values,width*sizeof(float));
        }
        acl::track_desc_scalarf d; d.output_index=i; d.precision=i==3 ? 0.0000001F : 0.0001F;
        raw[i]=track::make_copy(d,a,data.data(),frames,30.0F);
    }
    save(a,raw,acl::get_default_compression_settings(),dir+"/"+name,width);
}

void transforms(acl::ansi_allocator& a,const std::string& dir,const char* name,bool raw_format,uint32_t frames,
                bool local_constants=false,bool exact=false,bool mixed=false,bool looping=false,uint32_t count=9,bool no_scale=false,bool drop_full=false) {
    acl::track_array_qvvf raw(a,count);
    for(uint32_t i=0;i<count;++i) {
        std::vector<rtm::qvvf> data(frames);
        for(uint32_t f=0;f<frames;++f) {
            float angle=i==0 ? 0.0F : i==1 ? 0.25F : 0.03F*float(f)*(i+1);
            if(local_constants && i==2 && f<frames/2) angle=0.1F;
            if(mixed && i<7) angle=i==0 ? 0.0F : float(i)*0.15F;
            auto axis=rtm::vector_normalize3(rtm::vector_set(0.25F+0.1F*i,0.2F,1.0F));
            data[f].rotation=rtm::quat_from_axis_angle(axis,angle);
            float t=i==0 ? 0.0F : i==1 ? 7.0F : i==2 ? float(f)*0.25F : std::sin(float(f)*0.21F+i)*3.0F;
            if(local_constants && i==4 && f<frames/2) t=2.0F;
            if(mixed && i<6) t=float(i);
            data[f].translation=rtm::vector_set(t,t*0.25F+float(i),-t*0.5F);
            float s=i<3 ? 1.0F : i==3 ? 1.25F : 1.0F+std::cos(float(f)*0.1F+i)*0.1F;
            if(local_constants && i==5 && f<frames/2) s=1.25F;
            if(mixed && i%2) s=1.1F+float(i)*0.05F;
            data[f].scale=no_scale ? rtm::vector_set(1.0F,1.0F,1.0F) : rtm::vector_set(s,s+0.05F*(i%2),s-0.025F*(i%3));
        }
        if(looping) data[frames-1]=data[0];
        acl::track_desc_transformf d; d.output_index=i; d.parent_index=i==0 ? acl::k_invalid_track_index : 0;
        d.precision=exact ? 0.0F : 0.0001F;
        raw[i]=acl::track_qvvf::make_copy(d,a,data.data(),frames,30.0F);
    }
    auto settings=acl::get_default_compression_settings();
    settings.keyframe_stripping.strip_trivial=false;
    acl::qvvf_transform_error_metric metric; settings.error_metric=&metric;
    if(raw_format) {
        settings.rotation_format=acl::rotation_format8::quatf_full;
        settings.translation_format=acl::vector_format8::vector3f_full;
        settings.scale_format=acl::vector_format8::vector3f_full;
    }
    if(drop_full) settings.rotation_format=acl::rotation_format8::quatf_drop_w_full;
    save(a,raw,settings,dir+"/"+name,10);
}

int main(int argc,char** argv) {
    if(argc!=2) return 1;
    acl::ansi_allocator a;
    for(uint32_t count : {1,2,3,4,5,15,16,17}) {
        for(uint32_t frames : {1,11,37}) {
            for(uint32_t format : {0,1,2,3}) {
                std::string name="edge_"+std::to_string(count)+"_"+std::to_string(frames)+"_"+std::to_string(format);
                transforms(a,argv[1],name.c_str(),format==1,frames,false,false,false,false,count,format==3,format==2);
            }
        }
    }
    return 0;
}
