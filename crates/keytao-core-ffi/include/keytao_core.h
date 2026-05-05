#ifndef KEYTAO_CORE_H
#define KEYTAO_CORE_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Flat view of IME state returned to C callers.
 * All strings are null-terminated UTF-8. Free with keytao_free_state().
 */
typedef struct KeytaoState {
  char *preedit;
  uint32_t cursor;
  char **candidate_texts;
  char **candidate_comments;
  uint32_t candidate_count;
  uint32_t page;
  bool is_last_page;
  char *committed;
  char *select_keys;
} KeytaoState;

/**
 * Initialize the Rime engine. Must be called once before any other function.
 * Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
 * Returns true on success.
 */
bool keytao_init(const char *user_dir, const char *shared_dir);

bool keytao_is_initialized(void);

/**
 * Process a key event. Returns heap-allocated KeytaoState; caller must free
 * with keytao_free_state(). Returns null if the engine is not initialized.
 */
struct KeytaoState *keytao_process_key(uint32_t keyval, uint32_t modifiers);

/**
 * Select a candidate by 0-based index. Returns new state; caller must free.
 */
struct KeytaoState *keytao_select_candidate(uint32_t index);

/**
 * Flip to the next/previous candidate page. Returns new state; caller must free.
 */
struct KeytaoState *keytao_change_page(bool backward);

/**
 * Clear current composition (Escape). Returns new state; caller must free.
 */
struct KeytaoState *keytao_reset(void);

/**
 * Free a KeytaoState returned by any keytao_* function.
 */
void keytao_free_state(struct KeytaoState *ptr);

#endif  /* KEYTAO_CORE_H */
