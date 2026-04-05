#include "rind.h"
#include <stdio.h>
#include <unistd.h>

static void on_message(MessageContainer msg) {
  printf("Message recieved\n");
  printf("%s\n", msg.name);
}

int main(void) {
  TransportProtocol tp = init_tp(UDS, "/run/rind-tp/tp_demo@transport_state.sock");

  MessageContainer msg = create_message(State, Set);
  PayloadContainer payload = create_message_payload(String, "hello");
  set_message_payload(&msg, payload);
  set_message_name(&msg, "tp_demo@transport_state");

  listen_tp(&tp, &on_message);
  send_message(&tp, msg);

  for (;;) {
    // printf("Ping\n");
    sleep(1);
  }
  return 0;
}
