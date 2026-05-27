#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <dlfcn.h>

typedef struct {
  uint8_t protocol;
  const char** options;
  size_t len;
  uint64_t id;
} rind_tp;

typedef struct {
  uint8_t type;
  const char* content;
} rind_payload;

typedef struct {
  uint8_t type;
  uint8_t action;
  rind_payload* payload;
  const char* name;
} rind_msg;

typedef void (*rind_callback)(rind_msg);

typedef rind_tp (*fn_rind_init_tp)(uint8_t, const char*);
typedef void (*fn_rind_listen_tp)(rind_tp*, rind_callback);
typedef rind_msg (*fn_rind_log_msg)(const char*);
typedef rind_msg (*fn_rind_msg_enquire)(const char*, const char*);
typedef rind_msg (*fn_rind_enquiry_tp)(rind_tp*, rind_msg);
typedef uint8_t (*fn_rind_send_message)(const rind_tp*, rind_msg);

void print_output(rind_msg msg) {
  if (msg.name != NULL) {
    printf("Received message: %s\n", msg.name);
  } else {
    printf("Received message with null name!\n");
  }

  if (msg.payload != NULL) {
    printf("  Payload: %s\n", msg.payload->content);
  }
}

int main() {
  setvbuf(stdout, NULL, _IONBF, 0);
  printf("connecting to shm tp...\n");

  void* lib = dlopen("/lib/librind_api.so", RTLD_LAZY);
  if (!lib) {
    fprintf(stderr, "failed to load library: %s\n", dlerror());
    return 1;
  }

  fn_rind_init_tp rind_init_tp = (fn_rind_init_tp)dlsym(lib, "rind_init_tp");
  fn_rind_listen_tp rind_listen_tp = (fn_rind_listen_tp)dlsym(lib, "rind_listen_tp");
  fn_rind_log_msg rind_log_msg = (fn_rind_log_msg)dlsym(lib, "rind_log_msg");
  fn_rind_send_message rind_send_message = (fn_rind_send_message)dlsym(lib, "rind_send_message");
  fn_rind_msg_enquire rind_msg_enquire = (fn_rind_msg_enquire)dlsym(lib, "rind_msg_enquire");
  fn_rind_enquiry_tp rind_enquiry_tp = (fn_rind_enquiry_tp)dlsym(lib, "rind_enquiry_tp");

  if (!rind_init_tp || !rind_listen_tp || !rind_log_msg || !rind_send_message) {
    fprintf(stderr, "failed to find symbols: %s\n", dlerror());
    dlclose(lib);
    return 1;
  }

  const char* path = getenv("RIND_TP_SOCK");
  if (!path) {
    path = "/run/rind-tp/shm.sock";
  }

  rind_tp* tp = (rind_tp*)malloc(sizeof(rind_tp));
  *tp = rind_init_tp(2, path);
  printf("shm tp connected and mapped\n");

  rind_listen_tp(tp, print_output);

  printf("sending message to rind...\n");
  rind_msg msg = rind_log_msg("hello from shm client!");
  rind_send_message(tp, msg);

  rind_msg enq = rind_msg_enquire("has_state", "net:online");
  rind_msg resp = rind_enquiry_tp(tp, enq);
  print_output(resp);

  while (1) {
    sleep(1);
  }

  free(tp);
  dlclose(lib);
  return 0;
}
