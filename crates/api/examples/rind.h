#ifndef RIND_API_H
#define RIND_API_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef enum TransportProtocolMethod {
  STDIO = 0,
  UDS = 1,
} TransportProtocolMethod;

typedef enum MessageType {
  Signal = 0,
  State = 1,
  Enquiry = 2,
  Response = 3,
} MessageType;

typedef enum MessageAction {
  Remove = 0,
  Set = 1,
} MessageAction;

typedef enum PayloadType {
  String = 0,
  Json = 1,
} PayloadType;

typedef struct TransportProtocol {
  enum TransportProtocolMethod protocol;
  const char *const *options;
  uintptr_t len;
  uint64_t id;
} TransportProtocol;

typedef struct PayloadContainer {
  enum PayloadType type;
  const char *content;
} PayloadContainer;

typedef struct MessageContainer {
  enum MessageType type;
  enum MessageAction action;
  struct PayloadContainer *payload;
  const char *name;
} MessageContainer;

struct TransportProtocol init_tp(enum TransportProtocolMethod protocol, const char *options);

void listen_tp(struct TransportProtocol *tp, void (*func)(struct MessageContainer));

struct MessageContainer create_message(enum MessageType type, enum MessageAction action);

struct PayloadContainer create_message_payload(enum PayloadType type, const char *inner);

void set_message_payload(struct MessageContainer *message, struct PayloadContainer payload);

void set_message_name(struct MessageContainer *message, const char *name);

void send_message(const struct TransportProtocol *tp, struct MessageContainer message);

#endif  /* RIND_API_H */
