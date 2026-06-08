#include "rind.h"
#include <stdio.h>
#include <unistd.h>

void print_payload(const rind_payload *payload) {
  if (!payload) return;

  printf("  payload type: %d\n", rind_payload_get_type(payload));
  char *content = rind_payload_get_content(payload);
  printf("  payload content: %s\n", content ? content : "null");
  rind_free_string(content);
}

void print_msg(const rind_msg *msg) {
  if (!msg) return;

  printf("msg:\n");
  printf("  type: %d\n", rind_msg_get_type(msg));
  printf("  action: %d\n", rind_msg_get_action(msg));

  char *name = rind_msg_get_name(msg);
  printf("  name: %s\n", name ? name : "null");
  rind_free_string(name);

  const rind_payload *payload = rind_msg_get_payload(msg);
  if (payload) {
    print_payload(payload);
  } else {
    printf("  null\n");
  }
}

void print_invoke(const rind_invoke_cmd *cmd) {
  if (!cmd) return;

  printf("invoke_cmd:\n");
  printf("  type: %d\n", rind_invoke_cmd_get_type(cmd));

  char *action = rind_invoke_cmd_get_action(cmd);
  printf("  action: %s\n", action ? action : "null");
  rind_free_string(action);

  char *payload = rind_invoke_cmd_get_payload(cmd);
  printf("  payload: %s\n", payload ? payload : "null");
  rind_free_string(payload);
}

static void on_message(rind_msg *msg) {
  printf("\nreceived msg\n");
  print_msg(msg);
  printf("\n");
  rind_free_msg(msg);
}

int main(void) {
  rind_tp *tp = rind_init_tp(RIND_TP_METHOD_UDS, "/run/rind-tp/tp_demo:transport_state.sock");
  if (!tp) {
    fprintf(stderr, "failed to initialize transport\n");
    return 1;
  }

  rind_msg *msg = rind_create_msg(RIND_MSG_TYPE_FACET, RIND_MSG_ACTION_SET);
  rind_payload *payload = rind_create_msg_payload(RIND_PAYLOAD_TYPE_STRING, "hello");
  rind_set_message_payload(msg, payload);
  rind_set_message_name(msg, "tp_demo:transport_state");

  printf("\nmsg\n");
  print_msg(msg);
  printf("\n");

  rind_invoke_cmd *cmd = rind_create_invoke(RIND_INVOKE_TYPE_ENQUIRE, "list", "all");
  printf("\ninvoke request\n");
  print_invoke(cmd);
  printf("\n");

  rind_invoke_cmd *res = rind_invoke(cmd);
  if (res) {
    printf("\ninvoke respone\n");
    print_invoke(res);
    printf("\n");
    rind_free_invoke(res);
  }

  rind_listen_tp(tp, &on_message);
  rind_send_message(tp, msg);

  rind_free_msg(msg);
  rind_free_invoke(cmd);

  printf("\nlistening...\n");

  for (;;) {
    sleep(1);
  }

  rind_free_tp(tp);
  return 0;
}
