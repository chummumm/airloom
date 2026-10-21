#ifndef AIRLOOM_H
#define AIRLOOM_H

#ifdef __cplusplus
extern "C" {
#endif

/* All returned strings are UTF-8. The handle and each returned string have
   independent ownership. Do not destroy a handle while another call uses it.
   Functions never unwind into C++. Null handles are accepted as no-ops.
   Snapshot strings must be released with airloom_free_string, not free(). */
void *airloom_create(void);
void airloom_destroy(void *handle);
/* 0: queued, -1: invalid JSON/command, -2: busy or shutting down. */
int airloom_command(void *handle, const char *json);
char *airloom_snapshot(void *handle);
void airloom_free_string(char *value);

#ifdef __cplusplus
}
#endif
#endif

