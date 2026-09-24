#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef void *SEXP;
typedef SEXP (*RUnwindBody)(void *);
typedef void (*RUnwindCleanup)(void *, int);

enum {
    FACT_UNOBSERVED = 0,
    FACT_COMPLETED = 1,
    FACT_PARSE = 2,
    FACT_EVAL = 3,
    FACT_PRINT = 4,
    PHASE_PARSE = 1,
    PHASE_EVAL = 2,
    PHASE_PRINT = 3,
    INPUT_MODE_TOP_LEVEL = 0,
    INPUT_MODE_NESTED = 1,
    INPUT_EOF = 0,
    INPUT_TEXT = 1,
    INPUT_CANCELLED = 2,
    PROMPT_UNOBSERVED = 0,
    PROMPT_TOP_LEVEL = 1,
    PROMPT_NESTED = 2,
};

typedef struct {
    SEXP (*mk_string)(const char *);
    SEXP (*install)(const char *);
    SEXP (*find_var)(SEXP, SEXP);
    SEXP (*cons)(SEXP, SEXP);
    SEXP (*lcons)(SEXP, SEXP);
    SEXP (*eval)(SEXP, SEXP);
    SEXP (*protect)(SEXP);
    void (*unprotect)(int);
    int (*length)(SEXP);
    SEXP (*vector_elt)(SEXP, ptrdiff_t);
    int *(*logical)(SEXP);
    int *(*integer)(SEXP);
    int (*type_of)(SEXP);
    SEXP (*get_option1)(SEXP);
    SEXP (*string_elt)(SEXP, ptrdiff_t);
    const char *(*char_string)(SEXP);
    void (*print_value)(SEXP);
    int (*toplevel_exec)(void (*)(void *), void *);
    SEXP (*unwind_protect)(RUnwindBody, void *, RUnwindCleanup, void *, SEXP);
    void (*preserve_object)(SEXP);
    void (*release_object)(SEXP);
    SEXP nil_value;
    SEXP unbound_value;
    SEXP global_env;
    SEXP base_env;
} ArfRApi;

typedef int (*ArfTopLevelPromptCallback)(const char *, void *);
typedef int (*ArfInputCallback)(int, const char *, char *, int, int, char **, uint64_t *, void *);
typedef void (*ArfOutcomeCallback)(uint64_t, uint32_t, uint8_t, void *);
typedef int (*ArfReadConsole)(const char *, char *, int, int);

typedef struct {
    const ArfRApi *api;
    uint64_t command_id;
    uint32_t expression_id;
    uint8_t phase;
    uint8_t fact;
    uint8_t active;
    uint8_t armed;
    uint8_t running;
    const char *source;
    char *owned_source;
    SEXP parser;
    SEXP close_fn;
} CommandState;

typedef struct {
    const ArfRApi *api;
    ArfInputCallback input;
    ArfOutcomeCallback outcome;
    void *context;
    SEXP parser_factory;
    ArfTopLevelPromptCallback top_level_prompt;
} DriverState;

typedef struct {
    const ArfRApi *api;
    int result;
} PromptClassState;

typedef struct {
    const ArfRApi *api;
    const char *raw_prompt;
    int is_continuation;
    int options_are_ambiguous;
} PromptInfoState;

static DriverState driver;
static CommandState command;
static int skip_next_boundary;
static unsigned int input_callback_depth;
static ArfReadConsole legacy_read_console;

static SEXP call0(const ArfRApi *api, SEXP function);

static void classify_n_frame_body(void *data) {
    PromptClassState *state = (PromptClassState *)data;
    const ArfRApi *api = state->api;
    SEXP fn = api->find_var(api->install("sys.nframe"), api->base_env);
    SEXP value = call0(api, fn);
    api->protect(value);
    int *frame = api->integer(value);
    if (frame != NULL && api->length(value) == 1)
        state->result = *frame == 0 ? PROMPT_TOP_LEVEL : PROMPT_NESTED;
    api->unprotect(1);
}

static const char *string_option_value(const ArfRApi *api, SEXP option) {
    /* STRSXP is 16 in R's public SEXPTYPE enumeration. */
    if (option == NULL || api->type_of(option) != 16 || api->length(option) != 1)
        return NULL;
    SEXP element = api->string_elt(option, 0);
    if (element == NULL)
        return NULL;
    return api->char_string(element);
}

static void prompt_info_body(void *data) {
    PromptInfoState *state = (PromptInfoState *)data;
    const ArfRApi *api = state->api;
    SEXP main_option = api->get_option1(api->install("prompt"));
    api->protect(main_option);
    SEXP continuation_option = api->get_option1(api->install("continue"));
    api->protect(continuation_option);
    const char *main_prompt = string_option_value(api, main_option);
    const char *continuation_prompt = string_option_value(api, continuation_option);
    if (main_prompt != NULL && continuation_prompt != NULL &&
        strcmp(main_prompt, continuation_prompt) == 0)
        state->options_are_ambiguous = 1;
    if (continuation_prompt == NULL)
        continuation_prompt = "+ ";
    state->is_continuation = state->raw_prompt != NULL &&
                             strcmp(state->raw_prompt, continuation_prompt) == 0;
    api->unprotect(2);
}

static SEXP call0(const ArfRApi *api, SEXP function) {
    SEXP call = api->lcons(function, api->nil_value);
    api->protect(call);
    SEXP result = api->eval(call, api->global_env);
    api->unprotect(1);
    return result;
}

static SEXP call1(const ArfRApi *api, SEXP function, SEXP argument) {
    SEXP args = api->cons(argument, api->nil_value);
    api->protect(args);
    SEXP call = api->lcons(function, args);
    api->protect(call);
    SEXP result = api->eval(call, api->global_env);
    api->unprotect(2);
    return result;
}

int arf_repl_driver_classify_prompt(void) {
    PromptClassState state = { .api = driver.api, .result = PROMPT_UNOBSERVED };
    if (state.api == NULL || state.api->integer == NULL || state.api->toplevel_exec == NULL)
        return PROMPT_UNOBSERVED;
    if (state.api->toplevel_exec(classify_n_frame_body, &state) == 0)
        return PROMPT_UNOBSERVED;
    return state.result;
}

int arf_repl_driver_prompt_info(const ArfRApi *api, const char *raw_prompt,
                               int *is_continuation, int *options_are_ambiguous) {
    if (api == NULL || api->toplevel_exec == NULL || api->install == NULL ||
        api->get_option1 == NULL || api->type_of == NULL || api->length == NULL ||
        api->string_elt == NULL || api->char_string == NULL ||
        is_continuation == NULL || options_are_ambiguous == NULL)
        return 0;
    PromptInfoState state = {
        .api = api,
        .raw_prompt = raw_prompt,
        .is_continuation = 0,
        .options_are_ambiguous = 0,
    };
    if (api->toplevel_exec(prompt_info_body, &state) == 0)
        return 0;
    *is_continuation = state.is_continuation;
    *options_are_ambiguous = state.options_are_ambiguous;
    return 1;
}

static SEXP command_body(void *data) {
    CommandState *state = (CommandState *)data;
    const ArfRApi *api = state->api;
    state->running = 1;
    SEXP factory = driver.parser_factory;
    if (factory == NULL || factory == api->unbound_value) {
        state->fact = FACT_PARSE;
        return api->nil_value;
    }

    SEXP source = api->mk_string(state->source);
    api->protect(source);
    SEXP parser = call1(api, factory, source);
    api->protect(parser);
    api->preserve_object(parser);
    state->parser = parser;
    SEXP next_fn = api->vector_elt(parser, 0);
    state->close_fn = api->vector_elt(parser, 1);
    SEXP visible_fn = api->find_var(api->install("withVisible"), api->base_env);

    for (;;) {
        state->phase = PHASE_PARSE;
        SEXP parsed = call0(api, next_fn);
        api->protect(parsed);
        if (api->length(parsed) == 0) {
            api->unprotect(1);
            if (state->expression_id > 1)
                state->expression_id -= 1;
            break;
        }

        SEXP expression = api->vector_elt(parsed, 0);
        state->phase = PHASE_EVAL;
        SEXP args = api->cons(expression, api->nil_value);
        api->protect(args);
        SEXP visible_call = api->lcons(visible_fn, args);
        api->protect(visible_call);
        SEXP visible_result = api->eval(visible_call, api->global_env);
        api->protect(visible_result);
        SEXP value = api->vector_elt(visible_result, 0);
        SEXP is_visible = api->vector_elt(visible_result, 1);
        int *visible = api->logical(is_visible);
        if (visible != NULL && *visible != 0) {
            state->phase = PHASE_PRINT;
            api->print_value(value);
        }
        api->unprotect(4);
        state->expression_id += 1;
    }

    call0(api, state->close_fn);
    api->release_object(parser);
    state->parser = NULL;
    api->unprotect(2);
    return api->nil_value;
}

static void command_cleanup(void *data, int jump) {
    CommandState *state = (CommandState *)data;
    if (jump == 0 && state->fact == FACT_UNOBSERVED)
        state->fact = FACT_COMPLETED;
    else if (state->phase == PHASE_PARSE)
        state->fact = FACT_PARSE;
    else if (state->phase == PHASE_PRINT)
        state->fact = FACT_PRINT;
    else
        state->fact = FACT_EVAL;
    state->active = 0;
    state->armed = 0;
    state->running = 0;
    free(state->owned_source);
    state->owned_source = NULL;
    state->source = NULL;
}

static void invoke_close(void *data) {
    CommandState *state = (CommandState *)data;
    call0(state->api, state->close_fn);
}

static void finish_previous_command(void) {
    if (command.fact == FACT_UNOBSERVED)
        return;
    uint64_t command_id = command.command_id;
    uint32_t expression_id = command.expression_id;
    uint8_t fact = command.fact;
    SEXP parser = command.parser;
    SEXP close_fn = command.close_fn;
    command.fact = FACT_UNOBSERVED;
    command.parser = NULL;
    command.close_fn = NULL;
    if (parser != NULL) {
        CommandState close_state = command;
        close_state.close_fn = close_fn;
        command.api->toplevel_exec(invoke_close, &close_state);
        command.api->release_object(parser);
    }
    if (driver.outcome != NULL)
        driver.outcome(command_id, expression_id, fact, driver.context);
}

static void recover_unobserved_command(void) {
    if (command.active == 0 || command.armed == 0 || command.running != 0 ||
        command.fact != FACT_UNOBSERVED)
        return;
    uint64_t command_id = command.command_id;
    uint32_t expression_id = command.expression_id;
    command.active = 0;
    command.armed = 0;
    free(command.owned_source);
    command.owned_source = NULL;
    command.source = NULL;
    if (driver.outcome != NULL)
        driver.outcome(command_id, expression_id, FACT_UNOBSERVED, driver.context);
}

static int native_read_console(const char *prompt, char *buffer, int length, int history) {
    /* Windows installs this callback before R initialization. Until the REPL
       driver is configured after initialization, preserve the legacy input
       path through the original Rust callback. That callback returns before
       this C frame starts any jump-capable R evaluation. */
    if (driver.input == NULL)
        return legacy_read_console == NULL ? 0 :
               legacy_read_console(prompt, buffer, length, history);

    /* While R evaluation is running, or while Rust is still acquiring outer
       input, the request is nested by construction. An armed command that has
       not started (or has returned Unobserved) is still classified so the next
       real top-level boundary can recover it. */
    int prompt_class = PROMPT_NESTED;
    if (command.running == 0 && input_callback_depth == 0) {
        prompt_class = driver.top_level_prompt == NULL ? PROMPT_UNOBSERVED :
                       driver.top_level_prompt(prompt, driver.context);
    }
    if (prompt_class == PROMPT_UNOBSERVED)
        return 0;
    int top_level = prompt_class == PROMPT_TOP_LEVEL;
    if (top_level) {
        recover_unobserved_command();
        if (command.active == 0)
            finish_previous_command();
    }
    uint64_t command_id = 0;
    char *full_source = NULL;
    int mode = top_level ? INPUT_MODE_TOP_LEVEL : INPUT_MODE_NESTED;
    input_callback_depth += 1;
    int read = driver.input(mode, prompt, buffer, length, history, &full_source,
                            &command_id, driver.context);
    input_callback_depth -= 1;
    if (read == INPUT_EOF) {
        free(full_source);
        return 0;
    }
    if (read == INPUT_CANCELLED) {
        free(full_source);
        if (top_level && buffer != NULL && length > 1) {
            buffer[0] = '\n';
            buffer[1] = '\0';
            return 1;
        }
        return 0;
    }
    if (read != INPUT_TEXT) {
        free(full_source);
        return 0;
    }

    /* Nested R prompts receive their input directly; the outer command driver
       remains active and owns the eventual terminal fact. */
    if (!top_level || command.active != 0) {
        free(full_source);
        return read;
    }

    /* A top-level input must arrive as a complete C-owned source. R's buffer
       can be much shorter, so it carries only a dummy newline after eval. */
    if (full_source == NULL) {
        if (buffer != NULL && length > 1) {
            buffer[0] = '\n';
            buffer[1] = '\0';
            return 1;
        }
        return 0;
    }
    if (full_source[0] == '\0') {
        free(full_source);
        if (buffer != NULL && length > 1) {
            buffer[0] = '\n';
            buffer[1] = '\0';
            return 1;
        }
        return 0;
    }

    command.api = driver.api;
    command.command_id = command_id;
    command.expression_id = 1;
    command.phase = PHASE_PARSE;
    command.fact = FACT_UNOBSERVED;
    command.active = 1;
    command.armed = 1;
    command.running = 0;
    command.source = full_source;
    command.owned_source = full_source;
    command.parser = NULL;
    command.close_fn = NULL;
    if (skip_next_boundary != 0) {
        skip_next_boundary = 0;
        free(command.owned_source);
        command.owned_source = NULL;
        command.source = NULL;
    } else {
        driver.api->unwind_protect(command_body, &command, command_cleanup, &command, NULL);
    }

    if (buffer != NULL && length > 1) {
        buffer[0] = '\n';
        buffer[1] = '\0';
        return 1;
    }
    return 0;
}

/* Rust fills this allocation with UTF-8 bytes and a trailing NUL. Allocation
   and release stay in the same C runtime on Windows. */
char *arf_repl_driver_alloc_source(size_t length) {
    if (length == SIZE_MAX)
        return NULL;
    return (char *)malloc(length + 1);
}

int arf_repl_driver_read_console(const char *prompt, char *buffer, int length, int history) {
    return native_read_console(prompt, buffer, length, history);
}

void arf_repl_driver_set_legacy_read_console(ArfReadConsole callback) {
    legacy_read_console = callback;
}

void arf_repl_driver_test_skip_next_boundary(void) {
    skip_next_boundary = 1;
}

static void preserve_factory(void *data) {
    SEXP factory = *(SEXP *)data;
    driver.api->preserve_object(factory);
}

int arf_repl_driver_install(const ArfRApi *api, SEXP parser_factory,
                            ArfTopLevelPromptCallback top_level_prompt,
                            ArfInputCallback input, ArfOutcomeCallback outcome, void *context,
                            ArfReadConsole *destination) {
    if (api == NULL || parser_factory == NULL || top_level_prompt == NULL || input == NULL || destination == NULL)
        return 0;
    driver.api = api;
    if (api->toplevel_exec(preserve_factory, &parser_factory) == 0)
        return 0;
    driver.api = api;
    driver.input = input;
    driver.outcome = outcome;
    driver.context = context;
    driver.parser_factory = parser_factory;
    driver.top_level_prompt = top_level_prompt;
    *destination = native_read_console;
    return 1;
}
