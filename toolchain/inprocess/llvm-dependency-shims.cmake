# The official Windows static SDK records LibXml2 on the imported
# LLVMWindowsManifest target but does not ship libxml2. Oscan's strict LLD
# build removes that optional component, so a placeholder is sufficient while
# CMake imports the rest of LLVM's target graph.
if(NOT TARGET LibXml2::LibXml2)
  add_library(LibXml2::LibXml2 INTERFACE IMPORTED)
endif()
