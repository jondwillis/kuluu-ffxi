file(GLOB SOURCES Source/*.cpp)
add_library(Deterministic ${SOURCES})

add_library(RecastNavigation::Deterministic ALIAS Deterministic)
set_target_properties(Deterministic PROPERTIES DEBUG_POSTFIX -d)

set(Detour_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/Include")

if(RECASTNAVIGATION_DT_POLYREF64)
    target_compile_definitions(Deterministic PUBLIC DT_POLYREF64)
endif()
if(RECASTNAVIGATION_DT_VIRTUAL_QUERYFILTER)
    target_compile_definitions(Deterministic PUBLIC DT_VIRTUAL_QUERYFILTER)
endif()

target_include_directories(Deterministic PUBLIC
    "$<BUILD_INTERFACE:${Detour_INCLUDE_DIR}>"
)

set_target_properties(Deterministic PROPERTIES
        SOVERSION ${SOVERSION}
        VERSION ${LIB_VERSION}
        COMPILE_PDB_OUTPUT_DIRECTORY .
        COMPILE_PDB_NAME "Deterministic-d"
        )

install(TARGETS Deterministic
        EXPORT recastnavigation-targets
        RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
        ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
        LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT library
        INCLUDES DESTINATION ${CMAKE_INSTALL_INCLUDEDIR} ${CMAKE_INSTALL_INCLUDEDIR}/recastnavigation
        )

file(GLOB INCLUDES Include/*.h)
install(FILES ${INCLUDES} DESTINATION
    ${CMAKE_INSTALL_INCLUDEDIR}/recastnavigation)
if(MSVC)
    install(FILES "$<TARGET_FILE_DIR:Deterministic>/Deterministic-d.pdb" CONFIGURATIONS "Debug" DESTINATION "lib" OPTIONAL)
endif()
