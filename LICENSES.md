This software contains different units with different licenses, copyrights and authors.

| Unit | License | Copyright |
| - | - | - |
| /main/deepnest.js | GPLv3 | Deepnest (Jack Qiao) |
| /main/svgparser.js | MIT | Deepnest (Jack Qiao) |
| /main/util/clipper.js | Boost | Angus Johnson 2010-2014 |
| src/addon.cc, src/minkowski.cc | Boost | Copyright 2010 Intel Corporation; Copyright 2015 Jack Qiao |
| src/polygon | Boost | Copyright 2018 Glen Joseph Fernandes |
| /native/vendor/ironnest | MPL-2.0 | fork of jagua-rs (MPL-2.0) |
| /native/ironnest-napi | MPL-2.0 | Deepnest Server |
| /src, /server.mjs, /index.mjs, /cli.mjs | MIT | Deepnest Server |

`native/vendor/ironnest` and `native/ironnest-napi` are the optional ironnest
engine (MPL-2.0). The vendored source is a fork-and-extend of
[jagua-rs](https://github.com/JeroenGar/jagua-rs) (MPL-2.0, Jeroen Gardeyn), with
one documented local modification (`SeparationEffort::Off`) — see
[docs/IRONNEST_ENGINE.md](docs/IRONNEST_ENGINE.md).
