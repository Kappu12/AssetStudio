#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <iomanip>
#include <filesystem>
#include "acl/core/ansi_allocator.h"
#include "acl/compression/animation_clip.h"
#include "acl/compression/compression_settings.h"
#include "acl/algorithm/uniformly_sampled/encoder.h"
#include "acl/algorithm/uniformly_sampled/decoder.h"
#include "acl/decompression/default_output_writer.h"
using namespace acl;
int main(int argc,char**argv) {
 if(argc!=2)return 2;
 std::filesystem::create_directories(argv[1]);
 ANSIAllocator allocator;
 for(int mode=0;mode<5;mode++) {
  const uint32_t samples=mode==0?1:mode==4?37:11;
  const char* name=mode==0?"constant_raw":mode==1?"animated_raw":mode==2?"animated_drop":mode==3?"animated_variable":"multisegment_variable";
  RigidBone bone;
  bone.name=String(allocator,"root");bone.vertex_distance=1.0F;bone.parent_index=k_invalid_bone_index;bone.bind_transform=transform_identity_64();
  RigidSkeleton skeleton(allocator,&bone,1);
  AnimationClip clip(allocator,skeleton,samples,30u,String(allocator,name));
  for(uint32_t frame=0;frame<samples;frame++) {
   double x=mode==0?3.0:double(frame);
   auto& track=clip.get_bones()[0];
   track.rotation_track.set_sample(frame,quat_from_axis_angle(vector_set(0.0,1.0,0.0),0.04*(x+1)));
   track.translation_track.set_sample(frame,vector_set(1.0+x*0.125,2.0+x*x*0.01,3.0-x*0.03));
   track.scale_track.set_sample(frame,vector_set(1.0+x*0.01,0.9+x*0.02,1.1-x*0.005));
  }
  TransformErrorMetric metric;
  auto settings=mode>=3?get_default_compression_settings():CompressionSettings();
  settings.error_metric=&metric;settings.segmenting.enabled=mode==4;
  if(mode==2)settings.rotation_format=RotationFormat8::QuatDropW_96;
  OutputStats stats;
  CompressedClip* compressed=nullptr;
  auto error=uniformly_sampled::compress_clip(allocator,clip,settings,compressed,stats);
  if(!error.empty()||!compressed){std::cerr<<name<<": "<<error.c_str()<<"\n";return 3;}
  auto valid=compressed->is_valid(true);if(!valid.empty()){std::cerr<<valid.c_str()<<"\n";return 4;}
  auto stem=std::filesystem::path(argv[1])/name;
  std::ofstream binary(stem.string()+".acl",std::ios::binary);binary.write((const char*)compressed,compressed->get_size());binary.close();
  std::ofstream csv(stem.string()+".csv");csv<<std::setprecision(9);
  uniformly_sampled::DecompressionContext<uniformly_sampled::DebugDecompressionSettings> context;
  context.initialize(*compressed);
  Transform_32 output;
  DefaultOutputWriter writer(&output,1);
  for(uint32_t frame=0;frame<samples;frame++) {
   context.seek(float(frame)/30.0F,SampleRoundingPolicy::Nearest);context.decompress_pose(writer);
   csv<<frame<<','<<quat_get_x(output.rotation)<<','<<quat_get_y(output.rotation)<<','<<quat_get_z(output.rotation)<<','<<quat_get_w(output.rotation)
      <<','<<vector_get_x(output.translation)<<','<<vector_get_y(output.translation)<<','<<vector_get_z(output.translation)
      <<','<<vector_get_x(output.scale)<<','<<vector_get_y(output.scale)<<','<<vector_get_z(output.scale)<<'\n';
  }
  std::cout<<name<<" "<<compressed->get_size()<<" bytes "<<samples<<" samples\n";
  allocator.deallocate(compressed,compressed->get_size());
 }
}
