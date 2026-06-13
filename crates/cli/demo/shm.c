#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <dlfcn.h>

typedef enum {
  RIND_MSG_TYPE_IMPULSE = 0,
  RIND_MSG_TYPE_FACET = 1,
  RIND_MSG_TYPE_ENQUIRY = 2,
  RIND_MSG_TYPE_RESPONSE = 3,
  RIND_MSG_TYPE_UNKNOWN = 4,
} RIND_MSG_TYPE;

typedef enum {
  RIND_MSG_ACTION_REMOVE = 0,
  RIND_MSG_ACTION_SET = 1,
} RIND_MSG_ACTION;

typedef enum {
  RIND_PAYLOAD_TYPE_STRING = 0,
  RIND_PAYLOAD_TYPE_JSON = 1,
} RIND_PAYLOAD_TYPE;

typedef enum {
  RIND_TP_METHOD_STDIO = 0,
  RIND_TP_METHOD_UDS = 1,
  RIND_TP_METHOD_SHM = 2,
} RIND_TP_METHOD;

struct rind_msg;
struct rind_payload;
struct rind_tp;

typedef char* (*fn_rind_msg_get_name)(const struct rind_msg*);
typedef struct rind_payload* (*fn_rind_msg_get_payload)(const struct rind_msg*);
typedef char* (*fn_rind_payload_get_content)(const struct rind_payload*);

typedef void (*fn_rind_free_string)(char*);
typedef void (*fn_rind_free_msg)(struct rind_msg*);

typedef struct rind_tp* (*fn_rind_init_tp)(RIND_TP_METHOD, const char*);
typedef void (*fn_rind_listen_tp)(struct rind_tp*, void (*)(struct rind_msg*));
typedef struct rind_msg* (*fn_rind_enquiry_tp)(const struct rind_tp*, const struct rind_msg*);
typedef uint8_t (*fn_rind_send_message)(const struct rind_tp*, const struct rind_msg*);

typedef struct rind_msg* (*fn_rind_create_msg)(RIND_MSG_TYPE, RIND_MSG_ACTION);
typedef struct rind_payload* (*fn_rind_create_msg_payload)(RIND_PAYLOAD_TYPE, const char*);
typedef void (*fn_rind_set_message_payload)(struct rind_msg*, struct rind_payload*);
typedef void (*fn_rind_set_message_name)(struct rind_msg*, const char*);
typedef struct rind_msg* (*fn_rind_log_msg)(const char*);

fn_rind_msg_get_name rind_msg_get_name;
fn_rind_msg_get_payload rind_msg_get_payload;
fn_rind_payload_get_content rind_payload_get_content;
fn_rind_free_string rind_free_string;
fn_rind_free_msg rind_free_msg;
fn_rind_init_tp rind_init_tp;
fn_rind_listen_tp rind_listen_tp;
fn_rind_enquiry_tp rind_enquiry_tp;
fn_rind_send_message rind_send_message;
fn_rind_create_msg rind_create_msg;
fn_rind_create_msg_payload rind_create_msg_payload;
fn_rind_set_message_payload rind_set_message_payload;
fn_rind_set_message_name rind_set_message_name;
fn_rind_log_msg rind_log_msg;

void print_output(struct rind_msg* msg) {
  if (!msg) return;

  char* name = rind_msg_get_name(msg);
  if (name != NULL) {
    printf("Received message: %s\n", name);
    rind_free_string(name);
  } else {
    printf("Received message with null name!\n");
  }

  const struct rind_payload* payload = rind_msg_get_payload(msg);
  if (payload != NULL) {
    char* content = rind_payload_get_content(payload);
    if (content != NULL) {
      printf("  Payload: %s\n", content);
      rind_free_string(content);
    }
  }
}

int main() {
  setvbuf(stdout, NULL, _IONBF, 0);
  printf("connecting to shm tp...\n");

  void* lib = dlopen("/lib/librind_api.so", RTLD_NOW | RTLD_GLOBAL);
  if (!lib) {
    fprintf(stderr, "failed to load library: %s\n", dlerror());
    return 1;
  }

  rind_msg_get_name = (fn_rind_msg_get_name)dlsym(lib, "rind_msg_get_name");
  rind_msg_get_payload = (fn_rind_msg_get_payload)dlsym(lib, "rind_msg_get_payload");
  rind_payload_get_content = (fn_rind_payload_get_content)dlsym(lib, "rind_payload_get_content");
  rind_free_string = (fn_rind_free_string)dlsym(lib, "rind_free_string");
  rind_free_msg = (fn_rind_free_msg)dlsym(lib, "rind_free_msg");

  rind_init_tp = (fn_rind_init_tp)dlsym(lib, "rind_init_tp");
  rind_listen_tp = (fn_rind_listen_tp)dlsym(lib, "rind_listen_tp");
  rind_enquiry_tp = (fn_rind_enquiry_tp)dlsym(lib, "rind_enquiry_tp");
  rind_send_message = (fn_rind_send_message)dlsym(lib, "rind_send_message");

  rind_create_msg = (fn_rind_create_msg)dlsym(lib, "rind_create_msg");
  rind_create_msg_payload = (fn_rind_create_msg_payload)dlsym(lib, "rind_create_msg_payload");
  rind_set_message_payload = (fn_rind_set_message_payload)dlsym(lib, "rind_set_message_payload");
  rind_set_message_name = (fn_rind_set_message_name)dlsym(lib, "rind_set_message_name");
  rind_log_msg = (fn_rind_log_msg)dlsym(lib, "rind_log_msg");

  if (!rind_init_tp || !rind_listen_tp || !rind_send_message || !rind_create_msg) {
    fprintf(stderr, "failed to find symbols: %s\n", dlerror());
    dlclose(lib);
    return 1;
  }

  const char* path = getenv("RIND_TP_SOCK");
  if (!path) {
    path = "/run/rind-tp/shm.sock";
  }

  struct rind_tp* tp = rind_init_tp(RIND_TP_METHOD_SHM, path);
  printf("shm tp connected and mapped\n");

  rind_listen_tp(tp, print_output);

  printf("sending message to rind...\n");
  struct rind_msg* msg = rind_log_msg("hello from shm client!");
  rind_send_message(tp, msg);
  rind_free_msg(msg);

  struct rind_msg* enq = rind_create_msg(RIND_MSG_TYPE_ENQUIRY, RIND_MSG_ACTION_SET);
  struct rind_payload* payload = rind_create_msg_payload(RIND_PAYLOAD_TYPE_STRING, "net:online");

  rind_set_message_name(enq, "has_state");
  rind_set_message_payload(enq, payload);

  struct rind_msg* resp = rind_enquiry_tp(tp, enq);
  print_output(resp);

  rind_free_msg(enq);
  if (resp) {
    rind_free_msg(resp);
  }

  while (1) {
    sleep(1);
  }

  dlclose(lib);
  return 0;
}
