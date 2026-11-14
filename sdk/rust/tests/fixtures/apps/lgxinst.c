// lgxinst <pkg.lgx> <outdir>: what lgpm's installPlugin does with Basecamp's liblgx.
// Prints one line: gate=<package_valid> load=<0|1> variant=<v> extract=<0|1>, then
// writes <outdir>/<variant>/... (lgx_extract), <outdir>/<variant>/manifest.json
// (lgx_get_manifest_json) and <outdir>/<variant>/variant, like extractLgxPackage.
#include <stdio.h>
#include <string.h>
#include <stdbool.h>
#include <stddef.h>
#include "lgx.h"
int main(int argc, char** argv){
  if(argc<3){ fprintf(stderr,"usage: lgxinst pkg outdir\n"); return 2; }
  lgx_signature_info_t s=lgx_verify_signature(argv[1], NULL);
  printf("gate=%d ", s.package_valid);
  lgx_package_t p=lgx_load(argv[1]);
  if(!p){ printf("load=0 err=%s\n", lgx_get_last_error()); return 0; }
  printf("load=1 ");
  const char** sp=lgx_variant_spellings(lgx_host_variant());
  const char* v=NULL;
  for(const char** x=sp; x && *x; x++) if(lgx_has_variant(p,*x)){ v=*x; break; }
  if(!v){ printf("variant=- extract=0\n"); return 0; }
  printf("variant=%s ", v);
  lgx_result_t r=lgx_extract(p, v, argv[2]);
  printf("extract=%d", r.success);
  if(!r.success) printf(" err=%s", r.error?r.error:"");
  char path[4096];
  snprintf(path, sizeof path, "%s/%s/manifest.json", argv[2], v);
  const char* m=lgx_get_manifest_json(p);
  FILE* f=fopen(path,"wb"); if(f){ if(m) fwrite(m,1,strlen(m),f); fclose(f); }
  snprintf(path, sizeof path, "%s/%s/variant", argv[2], v);
  f=fopen(path,"wb"); if(f){ fputs(v,f); fclose(f); }
  printf("\n");
  return 0;
}
