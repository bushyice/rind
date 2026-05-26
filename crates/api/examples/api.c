#include "rind.h"
#include <stdio.h>
#include <unistd.h>

static void on_message(rind_msg msg) {
  printf("Message recieved\n");
  printf("%s\n", msg.name);
}

int main(void) {
  rind_tp tp = rind_init_tp(RIND_TP_METHOD_UDS, "/run/rind-tp/tp_demo:transport_state.sock");

  rind_msg msg = rind_create_msg(RIND_MSG_TYPE_FACET, RIND_MSG_ACTION_SET);
  rind_payload payload = rind_create_msg_payload(RIND_PAYLOAD_TYPE_STRING, "hello");
  rind_set_message_payload(&msg, payload);
  rind_set_message_name(&msg, "tp_demo:transport_state");

  rind_invoke_cmd cmd = rind_create_invoke(RIND_INVOKE_TYPE_ENQUIRE, "list", NULL);
  rind_invoke_cmd res = rind_invoke(cmd);

  printf("%s\n", res.payload);

  rind_listen_tp(&tp, &on_message);
  rind_send_message(&tp, msg);

  for (;;) {
    sleep(1);
  }
  return 0;
}
