#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <iomanip>
#include <filesystem>
#include <vector>
#include "acl/core/ansi_allocator.h"
#include "acl/compression/animation_clip.h"
#include "acl/compression/compression_settings.h"
#include "acl/algorithm/uniformly_sampled/encoder.h"
#include "acl/algorithm/uniformly_sampled/decoder.h"
#include "acl/decompression/default_output_writer.h"
using namespace acl;
struct Case {const char*name;uint16_t bones;uint32_t frames;int kind;};
int main(int argc,char**argv) {
 if(argc!=2)return 2;std::filesystem::create_directories(argv[1]);ANSIAllocator allocator;
 for(auto item:std::vector<Case>{{"constant_variable",1,1,0},{"default_variable",1,11,1},{"mixed_variable",3,11,2},{"multibone_variable",5,37,3},{"bitset_boundary_variable",12,11,2},{"no_scale_variable",2,37,4},{"segment_constant_variable",1,64,5},{"raw_escape_variable",1,11,6}}) {
  std::vector<RigidBone>bones(item.bones);
  for(uint16_t i=0;i<item.bones;i++){auto&bone=bones[i];auto name=std::string("bone")+std::to_string(i);bone.name=String(allocator,name.c_str());bone.vertex_distance=1.0F;bone.parent_index=i==0?k_invalid_bone_index:uint16_t(i-1);bone.bind_transform=transform_identity_64();}
  RigidSkeleton skeleton(allocator,bones.data(),item.bones);AnimationClip clip(allocator,skeleton,item.frames,30u,String(allocator,item.name));
  for(uint16_t bone=0;bone<item.bones;bone++)for(uint32_t frame=0;frame<item.frames;frame++) {
   bool identity=item.kind==1||(item.kind==2&&bone%3==2);
   bool constant=item.kind==0||(item.kind==2&&bone%3==1);
   double x=constant?3.0:double(frame);
   if(item.kind==5)x=frame<16?0.0:double(frame-15);
   double b=double(bone);
   auto&track=clip.get_bones()[bone];
   track.rotation_track.set_sample(frame,identity?quat_identity_64():quat_from_axis_angle(vector_set(0.0,1.0,0.0),0.02*(x+b+1.0)));
   track.translation_track.set_sample(frame,identity?vector_zero_64():vector_set(1.0+b+x*0.125,2.0+b+x*x*0.01,3.0-b-x*0.03));
   track.scale_track.set_sample(frame,identity||item.kind==4?vector_set(1.0):vector_set(1.0+x*0.01+b*0.1,0.9+x*0.02,1.1-x*0.005));
  }
  TransformErrorMetric metric;auto settings=get_default_compression_settings();settings.error_metric=&metric;settings.segmenting.enabled=item.frames>31;
  if(item.kind==6)settings.error_threshold=0.00000001F;
  OutputStats stats;CompressedClip*compressed=nullptr;auto error=uniformly_sampled::compress_clip(allocator,clip,settings,compressed,stats);
  if(!error.empty()||!compressed){std::cerr<<item.name<<": "<<error.c_str()<<"\n";return 3;}if(!compressed->is_valid(true).empty())return 4;
  auto stem=std::filesystem::path(argv[1])/item.name;std::ofstream binary(stem.string()+".acl",std::ios::binary);binary.write((const char*)compressed,compressed->get_size());binary.close();
  std::ofstream csv(stem.string()+".csv");csv<<std::setprecision(9);uniformly_sampled::DecompressionContext<uniformly_sampled::DebugDecompressionSettings>context;context.initialize(*compressed);
  std::vector<Transform_32>output(item.bones);DefaultOutputWriter writer(output.data(),item.bones);
  for(uint32_t frame=0;frame<item.frames;frame++){context.seek(float(frame)/30.0F,SampleRoundingPolicy::Nearest);context.decompress_pose(writer);csv<<frame;for(auto&v:output)csv<<','<<quat_get_x(v.rotation)<<','<<quat_get_y(v.rotation)<<','<<quat_get_z(v.rotation)<<','<<quat_get_w(v.rotation)<<','<<vector_get_x(v.translation)<<','<<vector_get_y(v.translation)<<','<<vector_get_z(v.translation)<<','<<vector_get_x(v.scale)<<','<<vector_get_y(v.scale)<<','<<vector_get_z(v.scale);csv<<'\n';}
  std::cout<<item.name<<" "<<compressed->get_size()<<" bytes "<<item.bones<<" bones "<<item.frames<<" frames\n";allocator.deallocate(compressed,compressed->get_size());
 }
}
