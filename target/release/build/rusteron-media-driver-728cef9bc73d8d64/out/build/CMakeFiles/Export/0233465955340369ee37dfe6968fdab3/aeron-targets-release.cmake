#----------------------------------------------------------------
# Generated CMake target import file for configuration "Release".
#----------------------------------------------------------------

# Commands may need to know the format version.
set(CMAKE_IMPORT_FILE_VERSION 1)

# Import target "aeron::aeron" for configuration "Release"
set_property(TARGET aeron::aeron APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(aeron::aeron PROPERTIES
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libaeron.dylib"
  IMPORTED_SONAME_RELEASE "@rpath/libaeron.dylib"
  )

list(APPEND _cmake_import_check_targets aeron::aeron )
list(APPEND _cmake_import_check_files_for_aeron::aeron "${_IMPORT_PREFIX}/lib/libaeron.dylib" )

# Import target "aeron::aeron_static" for configuration "Release"
set_property(TARGET aeron::aeron_static APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(aeron::aeron_static PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libaeron_static.a"
  )

list(APPEND _cmake_import_check_targets aeron::aeron_static )
list(APPEND _cmake_import_check_files_for_aeron::aeron_static "${_IMPORT_PREFIX}/lib/libaeron_static.a" )

# Import target "aeron::aeron_driver" for configuration "Release"
set_property(TARGET aeron::aeron_driver APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(aeron::aeron_driver PROPERTIES
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libaeron_driver.dylib"
  IMPORTED_SONAME_RELEASE "@rpath/libaeron_driver.dylib"
  )

list(APPEND _cmake_import_check_targets aeron::aeron_driver )
list(APPEND _cmake_import_check_files_for_aeron::aeron_driver "${_IMPORT_PREFIX}/lib/libaeron_driver.dylib" )

# Import target "aeron::aeron_driver_static" for configuration "Release"
set_property(TARGET aeron::aeron_driver_static APPEND PROPERTY IMPORTED_CONFIGURATIONS RELEASE)
set_target_properties(aeron::aeron_driver_static PROPERTIES
  IMPORTED_LINK_INTERFACE_LANGUAGES_RELEASE "C"
  IMPORTED_LOCATION_RELEASE "${_IMPORT_PREFIX}/lib/libaeron_driver_static.a"
  )

list(APPEND _cmake_import_check_targets aeron::aeron_driver_static )
list(APPEND _cmake_import_check_files_for_aeron::aeron_driver_static "${_IMPORT_PREFIX}/lib/libaeron_driver_static.a" )

# Commands beyond this point should not need to know the version.
set(CMAKE_IMPORT_FILE_VERSION)
