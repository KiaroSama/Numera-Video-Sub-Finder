# Adapted from microsoft/vcpkg e456309491fd875487bf02b8a0dca76c1b1b00cc (MIT).
# 5.5.4 includes the former MSVC <cstdint> patch; keep the remaining port fixes.
vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO tesseract-ocr/tesseract
    REF "${VERSION}"
    SHA512 c1dd838f8c9cbba585d969c1c5edba52d1aa77dac2057775271f5bb8e22c0550f723d1de362e15263e2c540ae1175b08bff36c5a545dd5f84906c468036b4556
    PATCHES
        fix_static_link_icu.patch
        fix-link-include-path.patch
        target-curl.diff
)

vcpkg_check_features(OUT_FEATURE_OPTIONS FEATURE_OPTIONS
    FEATURES
        training-tools BUILD_TRAINING_TOOLS
)
vcpkg_find_acquire_program(PKGCONFIG)
vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}"
    OPTIONS
        ${FEATURE_OPTIONS}
        -DUSE_SYSTEM_ICU=True
        -DCMAKE_REQUIRE_FIND_PACKAGE_LibArchive=ON
        -DCMAKE_REQUIRE_FIND_PACKAGE_CURL=ON
        -DCMAKE_REQUIRE_FIND_PACKAGE_Leptonica=ON
        -DCMAKE_DISABLE_FIND_PACKAGE_OpenCL=ON
        -DLeptonica_DIR=YES
        -DLEPT_TIFF_RESULT=ON
        "-DPKG_CONFIG_EXECUTABLE=${PKGCONFIG}"
    MAYBE_UNUSED_VARIABLES CMAKE_DISABLE_FIND_PACKAGE_OpenCL
)
vcpkg_cmake_install()
vcpkg_copy_pdbs()
vcpkg_cmake_config_fixup(CONFIG_PATH lib/cmake/tesseract)
vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/share/tesseract/TesseractConfig.cmake"
    "find_dependency(Leptonica)"
[[
find_dependency(CURL)
find_dependency(Leptonica)
find_dependency(LibArchive)
if(ANDROID)
    find_dependency(CpuFeaturesNdkCompat CONFIG)
endif()
]]
)
vcpkg_copy_tools(TOOL_NAMES tesseract AUTO_CLEAN)
vcpkg_fixup_pkgconfig()
if("training-tools" IN_LIST FEATURES)
    list(APPEND TRAINING_TOOLS
        ambiguous_words classifier_tester combine_tessdata cntraining dawg2wordlist
        mftraining shapeclustering wordlist2dawg combine_lang_model lstmeval lstmtraining
        text2image set_unicharset_properties unicharset_extractor merge_unicharsets
    )
    vcpkg_copy_tools(TOOL_NAMES ${TRAINING_TOOLS} AUTO_CLEAN)
endif()
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include" "${CURRENT_PACKAGES_DIR}/debug/share")
foreach(API_DIRECTORY IN ITEMS api arch ccmain ccstruct ccutil classify cutil dict lstm textord viewer wordrec)
    file(GLOB API_HEADER_FILES LIST_DIRECTORIES false "${SOURCE_PATH}/src/${API_DIRECTORY}/*.h")
    file(INSTALL ${API_HEADER_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/include/tesseract/${API_DIRECTORY}")
endforeach()
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE" "${CURRENT_PORT_DIR}/LICENSE.vcpkg.txt")
