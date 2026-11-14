{
  description = "Program Registry for Basecamp: browse the LEZ Program Registry";

  nixConfig = {
    extra-substituters = [ "https://cache.nix.logos.co/public" ];
    extra-trusted-public-keys = [ "public:l4HrXgL4nw246+LBh2SOJyhz64BoGegOYLheT/iIAPU=" ];
  };

  inputs = {
    # The builder revision Basecamp 0.3.x locks, the same as module/ (and the Names app).
    logos-module-builder.url = "github:logos-co/logos-module-builder/de169fd45d7bdf6e28ce1f151b4c34880523ace9";
    # The core module this view is a front end for. The input name must match the dependency
    # name in metadata.json. From a copy that is not a git checkout, pass
    # `--override-input program_registry path:/abs/path/to/module`.
    program_registry.url = "path:../module";
    program_registry.inputs.logos-module-builder.follows = "logos-module-builder";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
