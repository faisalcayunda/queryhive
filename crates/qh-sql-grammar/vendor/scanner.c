#include "tree_sitter/parser.h"
#include <stdlib.h>
#include <string.h>
#include <wctype.h>

enum TokenType {
  DOLLAR_QUOTED_STRING_START_TAG,
  DOLLAR_QUOTED_STRING_END_TAG,
  DOLLAR_QUOTED_STRING
};

#define MALLOC_STRING_SIZE 1024

typedef struct LexerState {
  char* start_tag;
} LexerState;

void *tree_sitter_sql_external_scanner_create(void) {
  LexerState *state = malloc(sizeof(LexerState));
  if (state == NULL) {
    return NULL;
  }
  state->start_tag = NULL;
  return state;
}

void tree_sitter_sql_external_scanner_destroy(void *payload) {
  LexerState *state = (LexerState*)payload;
  if (state == NULL) {
    return;
  }
  if (state->start_tag != NULL) {
    free(state->start_tag);
    state->start_tag = NULL;
  }
  free(payload);
}

// Appends the UTF-8 encoding of code point `c` to the NUL-terminated `text`, which holds
// `*length` bytes in a buffer of `*text_size`. Returns the (possibly moved) buffer, or NULL
// after freeing it when out of memory. `lookahead` is a code point, not a byte: narrowing it
// to `char` made `$Ł$` (U+0141) equal `$A$`. A negative value is tree-sitter's decode error;
// like surrogates and values past U+10FFFF it is stored as U+FFFD.
static char* add_char(char* text, size_t* text_size, size_t* length, int32_t c) {
  char bytes[4];
  size_t n;
  if (c < 0 || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) {
    c = 0xFFFD;
  }
  if (c < 0x80) {
    bytes[0] = (char)c;
    n = 1;
  } else if (c < 0x800) {
    bytes[0] = (char)(0xC0 | (c >> 6));
    bytes[1] = (char)(0x80 | (c & 0x3F));
    n = 2;
  } else if (c < 0x10000) {
    bytes[0] = (char)(0xE0 | (c >> 12));
    bytes[1] = (char)(0x80 | ((c >> 6) & 0x3F));
    bytes[2] = (char)(0x80 | (c & 0x3F));
    n = 3;
  } else {
    bytes[0] = (char)(0xF0 | (c >> 18));
    bytes[1] = (char)(0x80 | ((c >> 12) & 0x3F));
    bytes[2] = (char)(0x80 | ((c >> 6) & 0x3F));
    bytes[3] = (char)(0x80 | (c & 0x3F));
    n = 4;
  }

  if (text == NULL) {
    text = malloc(MALLOC_STRING_SIZE);
    if (text == NULL) {
      return NULL;
    }
    *text_size = MALLOC_STRING_SIZE;
    *length = 0;
  }

  // + 1 for the '\0'. One doubling always fits: n <= 4 and the buffer starts at 1024.
  if (*length + n + 1 > *text_size) {
    char* tmp = realloc(text, *text_size * 2);
    if (tmp == NULL) {
      free(text);
      return NULL;
    }
    text = tmp;
    *text_size *= 2;
  }

  memcpy(text + *length, bytes, n);
  *length += n;
  text[*length] = '\0';
  return text;
}

static char* scan_dollar_string_tag(TSLexer *lexer) {
  if (lexer->lookahead != '$') {
    return NULL;
  }

  size_t text_size = 0;
  size_t length = 0;
  char* tag = add_char(NULL, &text_size, &length, '$');
  if (tag == NULL) {
    return NULL;
  }
  lexer->advance(lexer, false);

  // A NUL ends the tag as a non-tag: it cannot live in a C string.
  while (lexer->lookahead != '$' && lexer->lookahead != 0 && !iswspace(lexer->lookahead) && !lexer->eof(lexer)) {
    tag = add_char(tag, &text_size, &length, lexer->lookahead);
    if (tag == NULL) {
      return NULL;
    }
    lexer->advance(lexer, false);
  }

  if (lexer->lookahead == '$') {
    tag = add_char(tag, &text_size, &length, lexer->lookahead);
    if (tag == NULL) {
      return NULL;
    }
    lexer->advance(lexer, false);
    return tag;
  } else {
    free(tag);
    return NULL;
  }
}

bool tree_sitter_sql_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  LexerState *state = (LexerState*)payload;
  if (state == NULL) {
    return false;
  }
  if (valid_symbols[DOLLAR_QUOTED_STRING_START_TAG] && state->start_tag == NULL) {
    while (iswspace(lexer->lookahead)) lexer->advance(lexer, true);

    char* start_tag = scan_dollar_string_tag(lexer);
    if (start_tag == NULL) {
      return false;
    }
    if (state->start_tag != NULL) {
      free(state->start_tag);
      state->start_tag = NULL;
    }
    state->start_tag = start_tag;
    lexer->result_symbol = DOLLAR_QUOTED_STRING_START_TAG;
    return true;
  }

  if (valid_symbols[DOLLAR_QUOTED_STRING_END_TAG] && state->start_tag != NULL) {
    while (iswspace(lexer->lookahead)) lexer->advance(lexer, true);

    char* end_tag = scan_dollar_string_tag(lexer);
    if (end_tag != NULL && strcmp(end_tag, state->start_tag) == 0) {
      free(state->start_tag);
      state->start_tag = NULL;
      lexer->result_symbol = DOLLAR_QUOTED_STRING_END_TAG;
      free(end_tag);
      return true;
    }
    if (end_tag != NULL) {
      free(end_tag);
    }
    return false;
  }

  if (valid_symbols[DOLLAR_QUOTED_STRING]) {
    lexer->mark_end(lexer);
    while (iswspace(lexer->lookahead)) lexer->advance(lexer, true);

    char* start_tag = scan_dollar_string_tag(lexer);
    if (start_tag == NULL) {
      return false;
    }

    if (state->start_tag != NULL && strcmp(state->start_tag, start_tag) == 0) {
      free(start_tag);
      return false;
    }

    char* end_tag = NULL;
    while (true) {
      if (lexer->eof(lexer)) {
        free(start_tag);
        free(end_tag);
        return false;
      }

      end_tag = scan_dollar_string_tag(lexer);
      if (end_tag == NULL) {
        lexer->advance(lexer, false);
        continue;
      }

      if (strcmp(end_tag, start_tag) == 0) {
        free(start_tag);
        free(end_tag);
        lexer->mark_end(lexer);
        lexer->result_symbol = DOLLAR_QUOTED_STRING;
        return true;
      }

      free(end_tag);
      end_tag = NULL;
    }
  }

  return false;
}

unsigned tree_sitter_sql_external_scanner_serialize(void *payload, char *buffer) {
  LexerState *state = (LexerState *)payload;
  if (state == NULL || state->start_tag == NULL) {
    return 0;
  }
  // + 1 for the '\0'
  size_t tag_length = strlen(state->start_tag) + 1;
  if (tag_length >= TREE_SITTER_SERIALIZATION_BUFFER_SIZE) {
    return 0;
  }

  memcpy(buffer, state->start_tag, tag_length);
  return (unsigned)tag_length;
}

void tree_sitter_sql_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  LexerState *state = (LexerState *)payload;
  if (state == NULL) {
    return;
  }
  if (state->start_tag != NULL) {
    free(state->start_tag);
    state->start_tag = NULL;
  }
  // A length of 1 can't exists.
  if (length > 1) {
    state->start_tag = malloc(length);
    if (state->start_tag == NULL) {
      return;
    }
    memcpy(state->start_tag, buffer, length);
  }
}
