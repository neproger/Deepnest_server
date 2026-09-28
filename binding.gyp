{
  "targets": [
    {
      "target_name": "opennest",
      "sources": [
        "src/opennest_addon.cc",
        "native/opennest/src/GeometryUtil.cpp",
        "native/opennest/src/MinkowskiConvolution.cpp",
        "native/opennest/src/NfpWorker.cpp",
        "native/opennest/src/GeneticAlgorithm.cpp",
        "native/opennest/src/NestingEngine.cpp",
        "native/opennest/src/NestingContext.cpp",
        "native/opennest/src/capi/nfp_nest_capi.cpp",
        "native/opennest/src/clipper2/clipper.engine.cpp",
        "native/opennest/src/clipper2/clipper.offset.cpp",
        "native/opennest/src/clipper2/clipper.rectclip.cpp"
      ],
      "defines": [
        "NAPI_VERSION=8",
        "_USE_MATH_DEFINES",
        "NOMINMAX"
      ],
      "include_dirs": [
        "<!(node -p \"require('node-addon-api').include_dir\")",
        "native/opennest/src",
        "native/opennest/third_party/boost_min"
      ],
      'cflags_cc': [ '-std=c++17', '-fexceptions' ],
      'cflags_cc!': [ '-fno-exceptions' ],
      "conditions": [
        [
          'OS=="win"', {
            'msvs_settings': {
              'VCCLCompilerTool': {
                'ExceptionHandling': 1,
                'AdditionalOptions': [ '/bigobj', '/W3', '/wd4244', '/wd4267', '/wd4305' ]
              }
            }
          }
        ],
        [
          'OS=="linux"', {
            'libraries': [ '-lpthread' ]
          }
        ],
        [
          'OS=="mac"', {
            'xcode_settings': {
              'CLANG_CXX_LANGUAGE_STANDARD': 'c++17',
              'GCC_ENABLE_CPP_EXCEPTIONS': 'YES'
            }
          }
        ]
      ]
    }
  ],
}
