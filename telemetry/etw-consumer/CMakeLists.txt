cmake_minimum_required(VERSION 3.15)
project(etw_consumer)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Find krabsetw (assumes it's installed or available)
# In production, you'd fetch it via FetchContent or vcpkg
find_path(KRABSETW_INCLUDE_DIR krabs.hpp
    PATHS 
        ${CMAKE_SOURCE_DIR}/third_party/krabsetw
        /usr/local/include
        /usr/include
)

if(NOT KRABSETW_INCLUDE_DIR)
    message(STATUS "krabsetw not found, will use placeholder")
endif()

add_executable(etw_consumer
    etw_consumer.cpp
)

if(KRABSETW_INCLUDE_DIR)
    target_include_directories(etw_consumer PRIVATE ${KRABSETW_INCLUDE_DIR})
    target_compile_definitions(etw_consumer PRIVATE HAS_KRABSETW)
endif()

# Windows-specific libraries
if(WIN32)
    target_link_libraries(etw_consumer 
        tdh
        advapi32
    )
endif()

install(TARGETS etw_consumer DESTINATION bin)
