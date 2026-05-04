#define _GNU_SOURCE
#include <dlfcn.h>
#include <netdb.h>
#include <string.h>
#include <stdlib.h>
#include <stdio.h>
#include <errno.h>

// Allowed domains are passed via RUNE_ALLOWED_DOMAINS env var (comma-separated)
static int is_domain_allowed(const char *node) {
    if (!node) return 1;  // NULL node = any, allow
    const char *env = getenv("RUNE_ALLOWED_DOMAINS");
    if (!env || strlen(env) == 0) return 0;  // empty = block all
    if (strcmp(env, "*") == 0) return 1;  // wildcard = allow all
    
    // Check each comma-separated domain
    char *copy = strdup(env);
    char *tok = strtok(copy, ",");
    while (tok) {
        // Exact match
        if (strcmp(node, tok) == 0) { free(copy); return 1; }
        // Wildcard subdomain match: *.example.com matches sub.example.com
        if (tok[0] == '*' && tok[1] == '.') {
            const char *suffix = tok + 1;  // ".example.com"
            size_t slen = strlen(suffix);
            size_t nlen = strlen(node);
            if (nlen > slen && strcmp(node + nlen - slen, suffix) == 0) {
                free(copy); return 1;
            }
        }
        tok = strtok(NULL, ",");
    }
    free(copy);
    return 0;
}

int getaddrinfo(const char *node, const char *service,
                const struct addrinfo *hints, struct addrinfo **res) {
    if (node && !is_domain_allowed(node)) {
        return EAI_NONAME;  // pretend domain doesn't exist
    }
    // Call real getaddrinfo
    int (*real_getaddrinfo)(const char*, const char*, const struct addrinfo*, struct addrinfo**);
    real_getaddrinfo = dlsym(RTLD_NEXT, "getaddrinfo");
    return real_getaddrinfo(node, service, hints, res);
}

struct hostent *gethostbyname(const char *name) {
    if (name && !is_domain_allowed(name)) {
        h_errno = HOST_NOT_FOUND;
        return NULL;
    }
    struct hostent* (*real_gethostbyname)(const char*);
    real_gethostbyname = dlsym(RTLD_NEXT, "gethostbyname");
    return real_gethostbyname(name);
}
