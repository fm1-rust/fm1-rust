cmake_minimum_required(VERSION 3.15)
include($ENV{PICO_SDK_PATH}/external/pico_sdk_import.cmake)
project(fm1_raw_host C CXX ASM)
pico_sdk_init()
find_package(Python3 REQUIRED COMPONENTS Interpreter)
set(pio "$ENV{PIO_USB_PATH}")
if(NOT TARGET pico_pio_usb)
  add_subdirectory("${pio}" pico-pio-usb)
endif()
add_custom_command(OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/vectors.h"
  COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/vectors.py" "${CMAKE_CURRENT_BINARY_DIR}/vectors.h"
  DEPENDS vectors.py VERBATIM)
add_executable(fm1_raw_host main.c "${CMAKE_CURRENT_BINARY_DIR}/vectors.h"
  "${pio}/examples/host_hid_to_device_cdc/usb_descriptors.c"
  "${PICO_TINYUSB_PATH}/src/portable/raspberrypi/pio_usb/hcd_pio_usb.c")
target_include_directories(fm1_raw_host PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}" "${CMAKE_CURRENT_BINARY_DIR}")
target_compile_definitions(fm1_raw_host PRIVATE PIO_USB_USE_TINYUSB)
target_link_libraries(fm1_raw_host PRIVATE pico_stdlib pico_pio_usb tinyusb_device tinyusb_host)
pico_add_extra_outputs(fm1_raw_host)
