// lgxview <pkg.lgx> <outdir>: what Basecamp's liblgx makes of a package. Writes
// <outdir>/manifest.json (lgx_get_manifest_json, the bytes lgpm installs) and extracts every
// variant to <outdir>/<variant>/. Prints "verify=<0|1>" and "load=ok|fail".
#include <stdio.h>
#include <string.h>
#include <stdbool.h>
#include <stddef.h>
#include "lgx.h"
int main(int argc, char** argv){
  if(argc<3){ fprintf(stderr,"usage: lgxview pkg outdir\n"); return 2; }
  lgx_verify_result_t r=lgx_verify(argv[1]);
  printf("verify=%d\n", r.valid);
  if(r.errors) for(const char** e=r.errors; *e; e++) printf("verify_error=%s\n", *e);
  lgx_package_t p=lgx_load(argv[1]);
  if(!p){ printf("load=fail %s\n", lgx_get_last_error()); return 1; }
  printf("load=ok\n");
  char path[4096];
  snprintf(path, sizeof path, "%s/manifest.json", argv[2]);
  const char* m=lgx_get_manifest_json(p);
  FILE* f=fopen(path,"wb"); if(f){ if(m) fwrite(m,1,strlen(m),f); fclose(f); }
  const char** vs=lgx_get_variants(p);
  for(const char** v=vs; v && *v; v++){
    lgx_result_t x=lgx_extract(p, *v, argv[2]);
    printf("variant=%s extract=%d %s\n", *v, x.success, x.error?x.error:"");
  }
  return 0;
}
