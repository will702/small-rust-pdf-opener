// Build with clang++ -std=c++17 -I <LibreOffice include directory>.
// Run in a subprocess: office_probe <Frameworks directory> <input.docx> <output prefix>.
#define LOK_USE_UNSTABLE_API
#include <LibreOfficeKit/LibreOfficeKit.hxx>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

int main(int argc, char** argv) {
    if (argc != 4) return 2;
    setenv("SAL_LOK_OPTIONS", "unipoll", 1);
    auto* kit = lok_init_2(argv[1], "file:///tmp/pdf-opener-office-probe-profile");
    if (!kit) return 3;
    lok::Office office(kit);
    std::puts("Initialized"); std::fflush(stdout);
    auto* doc = office.documentLoad(argv[2]);
    if (!doc) { std::fprintf(stderr, "%s\n", office.getError()); return 4; }
    std::puts("Loaded DOCX"); std::fflush(stdout);
    doc->initializeForRendering();
    long width = 0, height = 0;
    doc->getDocumentSize(&width, &height);
    if (width <= 0 || height <= 0) return 5;
    std::vector<unsigned char> pixels(512 * 512 * 4);
    doc->paintTile(pixels.data(), 512, 512, 0, 0, width, width);
    std::puts("Rendered tile"); std::fflush(stdout);
    doc->postUnoCommand(".uno:SelectAll");
    if (!doc->paste("text/plain;charset=utf-8", "Edited locally", 14)) return 6;
    doc->postUnoCommand(".uno:Undo");
    const std::string out(argv[3]);
    if (!doc->saveAs((out + ".docx").c_str(), "docx")) return 7;
    if (!doc->saveAs((out + ".pdf").c_str(), "pdf")) return 8;
    std::puts("Saved DOCX and PDF; GUI/IME and fidelity still require validation");
    delete doc;
}
