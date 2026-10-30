// Scratch-only development oracle. Not part of AssetStudio or its runtime.
// Map one trusted repository x64 PE DLL and supply bounded CRT ABI thunks.
#define _GNU_SOURCE
#include <sys/mman.h>
#include <sys/resource.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <math.h>
#include <errno.h>
#include <signal.h>
#include <ucontext.h>
#define MS __attribute__((ms_abi))
static void die(const char *s) { fprintf(stderr,"oracle: %s\n",s); _exit(120); }
static uint8_t *image; static size_t image_len;
static void fault(int sig,siginfo_t*info,void*ctx) {ucontext_t*u=ctx;char buffer[384];int n=snprintf(buffer,sizeof(buffer),"oracle signal %d address=%p rip=%llx image_rva=%llx rcx=%llx rdx=%llx rax=%llx\n",sig,info->si_addr,(unsigned long long)u->uc_mcontext.gregs[REG_RIP],(unsigned long long)(u->uc_mcontext.gregs[REG_RIP]-(uintptr_t)image),(unsigned long long)u->uc_mcontext.gregs[REG_RCX],(unsigned long long)u->uc_mcontext.gregs[REG_RDX],(unsigned long long)u->uc_mcontext.gregs[REG_RAX]);ssize_t written=write(2,buffer,n);(void)written;_exit(128+sig);}
static void *area(size_t off,size_t n) { if(off>image_len||n>image_len-off) die("PE image region out of bounds"); return image+off; }
static uint16_t get16(const void *p) {uint16_t n;memcpy(&n,p,2);return n;}
static uint32_t get32(const void *p) {uint32_t n;memcpy(&n,p,4);return n;}
static uint64_t get64(const void *p) {uint64_t n;memcpy(&n,p,8);return n;}
static char *name(size_t off) {char *s=area(off,1);size_t max=image_len-off;if(max>4096)max=4096;if(!memchr(s,0,max))die("unterminated PE name");return s;}
struct allocation { void *p;size_t len;int live; };
static struct allocation allocations[8192];static size_t allocated,allocation_count;
static void record(void *p,size_t n) {if(!p||allocation_count>=8192)die("allocation failed/too many allocations");allocations[allocation_count++]=(struct allocation){p,n,1};}
static void check_size(size_t n) {if(n>128*1024*1024||allocated>256*1024*1024-n)die("CRT shared allocation limit");allocated+=n;}
static void MS *aligned_malloc_ms(size_t n,size_t alignment) {check_size(n);void *p=0;if(alignment<sizeof(void*))alignment=sizeof(void*);if(alignment>65536||(alignment&(alignment-1)))die("bad CRT alignment");if(posix_memalign(&p,alignment,n?n:1))die("aligned allocation failed");record(p,n);return p;}
static void MS *malloc_ms(size_t n) {check_size(n);void*p=malloc(n?n:1);record(p,n);return p;}
static void MS free_ms(void *p) {if(!p)return;for(size_t i=0;i<allocation_count;i++)if(allocations[i].live&&allocations[i].p==p){allocations[i].live=0;free(p);return;}die("unowned/double free");}
static void MS *memcpy_ms(void*d,const void*s,size_t n) {if(n>128*1024*1024)die("memcpy work limit");return memcpy(d,s,n);}
static void MS *memmove_ms(void*d,const void*s,size_t n) {if(n>128*1024*1024)die("memmove work limit");return memmove(d,s,n);}
static void MS *memset_ms(void*d,int c,size_t n) {if(n>128*1024*1024)die("memset work limit");return memset(d,c,n);}
static float MS floorf_ms(float x) {return floorf(x);}
static void MS unavailable_ms(void) {die("unsupported Windows/exception import was invoked");}
static void *import_address(const char*s) {
 if(!strcmp(s,"_aligned_malloc"))return aligned_malloc_ms;
 if(!strcmp(s,"malloc"))return malloc_ms;
 if(!strcmp(s,"_aligned_free")||!strcmp(s,"free"))return free_ms;
 if(!strcmp(s,"memcpy"))return memcpy_ms;
 if(!strcmp(s,"memmove"))return memmove_ms;
 if(!strcmp(s,"memset"))return memset_ms;
 if(!strcmp(s,"floorf"))return floorf_ms;
 return unavailable_ms;
}
static uint8_t *read_file(const char *path,size_t *len) {FILE*f=fopen(path,"rb");if(!f)die("cannot open input");if(fseek(f,0,SEEK_END))die("seek input");long size=ftell(f);if(size<0||size>16*1024*1024)die("input file limit");rewind(f);uint8_t*p=malloc(size?size:1);if(fread(p,1,size,f)!=(size_t)size)die("read input");fclose(f);*len=size;return p;}
static void load_image(const char *path,uint32_t *export_dir) {
 size_t n;uint8_t*b=read_file(path,&n);
 if(n<64||get16(b)!=0x5a4d)die("not a PE file");
 size_t pe=get32(b+0x3c);if(pe>n||n-pe<24||get32(b+pe)!=0x4550||get16(b+pe+4)!=0x8664)die("not an x64 PE DLL");
 size_t opt=pe+24,opts=get16(b+pe+20);if(opt>n||opts>n-opt||opts<160||get16(b+opt)!=0x20b)die("bad PE optional header");
 image_len=get32(b+opt+56);if(image_len<4096||image_len>32*1024*1024)die("PE image size limit");
 uint64_t preferred=get64(b+opt+24);
 image=mmap((void*)(uintptr_t)preferred,image_len,PROT_READ|PROT_WRITE|PROT_EXEC,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE,-1,0);
 if(image==MAP_FAILED)image=mmap(0,image_len,PROT_READ|PROT_WRITE|PROT_EXEC,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
 if(image==MAP_FAILED)die("map PE image");
 size_t headers=get32(b+opt+60);if(headers>n)die("PE headers out of bounds");memcpy(area(0,headers),b,headers);
 size_t sections=get16(b+pe+6),table=opt+opts;if(sections>64||table>n||sections*40>n-table)die("section table out of bounds");
 for(size_t i=0;i<sections;i++){uint8_t*s=b+table+i*40;size_t va=get32(s+12),size=get32(s+16),off=get32(s+20);if(off>n||size>n-off)die("PE section exceeds file");memcpy(area(va,size),b+off,size);}
 uint64_t delta=(uintptr_t)image-preferred;
 size_t reloc=get32(b+opt+152),reloc_len=get32(b+opt+156);if(reloc_len){size_t end=reloc+reloc_len;area(reloc,reloc_len);while(reloc<end){uint8_t*r=area(reloc,8);size_t page=get32(r),size=get32(r+4);if(size<8||size>end-reloc||size%2)die("bad relocation block");for(size_t off=8;off<size;off+=2){uint16_t e=get16(area(reloc+off,2));if((e>>12)==10){uint8_t*p=area(page+(e&4095),8);uint64_t v=get64(p)+delta;memcpy(p,&v,8);}else if((e>>12)!=0)die("unsupported relocation type");}reloc+=size;}}
 size_t imports=get32(b+opt+120),import_len=get32(b+opt+124);if(import_len){area(imports,import_len);for(size_t i=0;i<128;i++){uint8_t*d=area(imports+i*20,20);size_t source=get32(d),iat=get32(d+16);if(!source&&!iat)break;for(size_t j=0;j<4096;j++){uint64_t thunk=get64(area(source+j*8,8));if(!thunk)break;if(thunk>>63)die("ordinal import unsupported");char*s=name(thunk+2);void*fn=import_address(s);memcpy(area(iat+j*8,8),&fn,8);}}}
 *export_dir=get32(b+opt+112);free(b);
}
static void *symbol(uint32_t exports,const char *wanted) {uint8_t*d=area(exports,40);size_t count=get32(d+24),names=get32(d+32),ords=get32(d+36),fns=get32(d+28);if(count>4096)die("too many exports");for(size_t i=0;i<count;i++){if(!strcmp(name(get32(area(names+i*4,4))),wanted)){size_t ord=get16(area(ords+i*2,2));uint32_t rva=get32(area(fns+ord*4,4));return area(rva,1);}}return 0;}
static void *guarded_input(const char *path) {size_t n;uint8_t*file=read_file(path,&n);size_t data=(n+64+4095)&~(size_t)4095;uint8_t*map=mmap(0,data+8192,PROT_NONE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);if(map==MAP_FAILED||mprotect(map+4096,data,PROT_READ|PROT_WRITE))die("guarded input mapping");memcpy(map+4096,file,n);free(file);return map+4096;}
typedef struct {float*values;int32_t values_count;float*times;int32_t times_count;} Clip;
static void validate_output(float*p,int32_t n) {if(n<0||n>8000000)die("output count limit");if(!n)return;if(!p)die("null output pointer");size_t size=(size_t)n*4;int owned=0;for(size_t i=0;i<allocation_count;i++){struct allocation*a=&allocations[i];if(a->live&&(uintptr_t)p>=(uintptr_t)a->p&&(uintptr_t)p-(uintptr_t)a->p<=a->len&&size<=a->len-((uintptr_t)p-(uintptr_t)a->p))owned=1;}if(!owned)die("output exceeds a live owned allocation");for(int32_t i=0;i<n;i++)if(!isfinite(p[i]))die("nonfinite output");}
int main(int argc,char **argv) {
 if(argc!=3&&argc!=4){fprintf(stderr,"usage: acl-pe-oracle dll data [database]\n");return 1;}
 struct rlimit cpu={3,3},memory={768*1024*1024,768*1024*1024},core={0,0};setrlimit(RLIMIT_CPU,&cpu);setrlimit(RLIMIT_AS,&memory);setrlimit(RLIMIT_CORE,&core);
 struct sigaction handler={0};handler.sa_sigaction=fault;handler.sa_flags=SA_SIGINFO;sigaction(SIGSEGV,&handler,0);sigaction(SIGBUS,&handler,0);sigaction(SIGILL,&handler,0);
 uint32_t exports;load_image(argv[1],&exports);void*data=guarded_input(argv[2]);Clip clip={0};
 typedef void(MS *D)(Clip*);D dispose=(D)symbol(exports,"Dispose");if(!dispose)die("missing Dispose");dispose(&clip);fprintf(stderr,"loader preflight: Dispose(empty) succeeded\n");
 if(argc==4){void*db=guarded_input(argv[3]);typedef void(MS *F)(const void*,const void*,Clip*);F decode=(F)symbol(exports,"DecompressTracks");if(!decode)die("missing DecompressTracks");decode(data,db,&clip);}
 else{typedef void(MS *F)(const void*,Clip*);F decode=(F)symbol(exports,"DecompressAll");if(!decode)die("missing DecompressAll");decode(data,&clip);}
 validate_output(clip.values,clip.values_count);validate_output(clip.times,clip.times_count);
 printf("%d,%d\n",clip.values_count,clip.times_count);for(int32_t i=0;i<clip.times_count;i++)printf(i?",%.9g":"%.9g",clip.times[i]);putchar('\n');for(int32_t i=0;i<clip.values_count;i++)printf(i?",%.9g":"%.9g",clip.values[i]);putchar('\n');
 dispose(&clip);
 return 0;
}
