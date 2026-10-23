/* PDFKit.dll — PDF parsing, text extraction, and generation behind
 * one clean C ABI. All strings UTF-8, NUL-terminated. char* results
 * are caller-owned: free with pk_free_string(). Byte buffers from
 * pk_render are freed with pk_free_buffer(). Documents (parsed) and
 * generators (being built) live behind uint64_t handles; close with
 * pk_close(). Page indices are 0-based. Draw coordinates are PDF
 * points with the origin at the BOTTOM-left of the page. */
#ifndef PDFKIT_H
#define PDFKIT_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#if defined(_WIN32) && defined(PK_BUILD)
#define PK_API __declspec(dllexport)
#elif defined(_WIN32)
#define PK_API __declspec(dllimport)
#else
#define PK_API
#endif
#define PK_OK 0
#define PK_ERR_INVALID_ARG 1
#define PK_ERR_NOT_SUPPORTED 2
#define PK_ERR_NOT_FOUND 3
#define PK_ERR_ACCESS_DENIED 4
#define PK_ERR_PLATFORM 5
#define PK_ERR_INTERNAL 6
/* Extension beyond the shared 0-6 set: invalid/unsupported PDF data. */
#define PK_ERR_CORRUPT 7

PK_API const char *pk_version(void);
PK_API int pk_version_major(void);
PK_API int pk_version_minor(void);
PK_API int pk_version_patch(void);
PK_API const char *pk_last_error(void);
PK_API int pk_last_error_code(void);
PK_API void pk_free_string(char *p);
PK_API void pk_free_buffer(uint8_t *p, size_t len);

/* Reading. */
PK_API uint64_t pk_open_file(const char *path);
PK_API uint64_t pk_open_bytes(const uint8_t *data, size_t len);
PK_API int pk_close(uint64_t handle);
PK_API int pk_page_count(uint64_t handle); /* -1 on error */
PK_API char *pk_info_json(uint64_t handle);
PK_API char *pk_page_size(uint64_t handle, uint32_t page);
PK_API char *pk_page_text(uint64_t handle, uint32_t page);

/* Writing (generator handles). */
PK_API uint64_t pk_create(void);
PK_API int pk_is_generator(uint64_t handle); /* 1 = generator, 0 = document/other */
PK_API int pk_set_info(uint64_t handle, const char *title, const char *author);
PK_API int pk_add_page(uint64_t handle, double width, double height); /* page index or -1 */
PK_API int pk_draw_text(uint64_t handle, uint32_t page, double x, double y,
                        double font_size, const char *text);
PK_API int pk_render(uint64_t handle, uint8_t **out_ptr, size_t *out_len);
PK_API int pk_save(uint64_t handle, const char *path);

#ifdef __cplusplus
}
#endif
#endif
