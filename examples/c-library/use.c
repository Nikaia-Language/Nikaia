/* examples/c-library/use.c - a C program using the library.
 *
 *   nikaia build
 *   cc -I target/nikaia/c-library use.c -L target/nikaia/c-library -lwordtally -o use
 *   LD_LIBRARY_PATH=target/nikaia/c-library ./use
 */
#include <stdio.h>
#include <string.h>
#include "wordtally.h"

static bool print_word(const uint8_t *word, size_t len, wordtally_Case c, void *ctx) {
    int *left = ctx;
    const char *cases[] = {"lower", "upper", "mixed"};
    printf("  %.*s (%s)\n", (int)len, (const char *)word, cases[c]);
    return --*left > 0;
}

int main(void) {
    const char *lines[] = {"the quick brown fox", "jumps over the lazy dog"};
    wordtally_Tally *tally = NULL;
    if (wordtally_Tally_new(&tally) != WORDTALLY_OK) {
        return 1;
    }
    int64_t words = 0;
    for (size_t i = 0; i < 2; i++) {
        wordtally_Tally_add(tally, (const uint8_t *)lines[i], strlen(lines[i]), &words);
    }
    char longest[32];
    size_t written = 0;
    int status = wordtally_Tally_longest(tally, (uint8_t *)longest, sizeof longest, &written);
    printf("%lld words, the longest \"%.*s\" (status %d)\n", (long long)words, (int)written, longest, status);
    wordtally_Tally_free(tally);

    const char *shout = "Hello NIKAIA from c";
    int left = 2;
    int64_t seen = 0;
    printf("the first two words of \"%s\":\n", shout);
    wordtally_each_word((const uint8_t *)shout, strlen(shout), print_word, &left, &seen);
    printf("stopped after %lld\n", (long long)seen);
    return 0;
}
