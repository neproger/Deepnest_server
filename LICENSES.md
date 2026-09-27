This software contains different units with different licenses, copyrights and authors.

| Unit | License | Copyright|
| - | - | - |
| /main/deepnest.js | GPLv3 | Deepnest (Jack Qiao) |
| /main/svgparser.js | MIT | Deepnest (Jack Qiao) |
| /main/util/clipper.js | Boost | Angus Johnson 2010-2014 |
| /native/vendor/ironnest | MPL-2.0 | fork of jagua-rs (MPL-2.0) |
| /native/ironnest-napi | MPL-2.0 | Deepnest Server |
| /src, /server.mjs, /index.mjs, /cli.mjs | MIT | Deepnest Server |

`native/vendor/ironnest` is a fork-and-extend of
[jagua-rs](https://github.com/JeroenGar/jagua-rs) (MPL-2.0, Jeroen Gardeyn) and
is licensed MPL-2.0 (file-scoped copyleft). The vendored source is unmodified
except for the documented `SeparationEffort::Off` addition — see
[docs/IRONNEST_ENGINE.md](docs/IRONNEST_ENGINE.md).