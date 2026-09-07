# MLX 0.25.1 still uses a default metallib for attention/normalization with JIT.
# Load those bytes from the linked Rust crate rather than a build-directory file.
set(device_source "${mlx_SOURCE_DIR}/mlx/backend/metal/device.cpp")
file(READ "${device_source}" contents)
string(FIND "${contents}" "extern \"C\" const unsigned char* wgo_mlx_metallib" begin)
if(begin EQUAL -1)
  string(FIND "${contents}" "MTL::Library* load_default_library(MTL::Device* device) {" begin)
endif()
string(FIND "${contents}" "\nMTL::Library* load_library(" end)
if(begin EQUAL -1 OR end LESS begin)
  message(FATAL_ERROR "MLX library loader changed; review the embedded metallib patch")
endif()
string(SUBSTRING "${contents}" 0 ${begin} prefix)
string(SUBSTRING "${contents}" ${end} -1 suffix)
set(loader [=[
extern "C" const unsigned char* wgo_mlx_metallib(size_t* size);
MTL::Library* load_default_library(MTL::Device* device) {
  size_t size = 0;
  const unsigned char* bytes = wgo_mlx_metallib(&size);
  dispatch_data_t data = dispatch_data_create(
      bytes, size, nullptr, DISPATCH_DATA_DESTRUCTOR_DEFAULT);
  NS::Error* error = nullptr;
  auto library = device->newLibrary(data, &error);
  dispatch_release(data);
  if (!library) {
    throw std::runtime_error("Failed to load embedded MLX metallib");
  }
  return library;
}
]=])
set(patched "${prefix}${loader}${suffix}")
if(NOT patched STREQUAL contents)
  file(WRITE "${device_source}" "${patched}")
endif()
