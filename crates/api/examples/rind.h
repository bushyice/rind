/*
 * Copyright (c) 2026 rind contributors
 *
 * This header is provided under the MIT License.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef enum TransportProtocolMethod {
  TransportProtocolMethod_STDIO = 0,
  TransportProtocolMethod_UDS = 1,
} TransportProtocolMethod;

typedef enum MessageType {
  MessageType_Signal = 0,
  MessageType_State = 1,
  MessageType_Enquiry = 2,
  MessageType_Response = 3,
} MessageType;

typedef enum MessageAction {
  MessageAction_Remove = 0,
  MessageAction_Set = 1,
} MessageAction;

typedef enum PayloadType {
  PayloadType_String = 0,
  PayloadType_Json = 1,
} PayloadType;

typedef enum InvokeType {
  InvokeType_Valid = 0,
  InvokeType_Ok = 1,
  InvokeType_Error = 2,
  InvokeType_Unknown = 3,
  InvokeType_RequestInput = 4,
  InvokeType_Enquire = 5,
} InvokeType;

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

typedef struct InvokeCommand {
  enum InvokeType type;
  const char *action;
  const char *payload;
} InvokeCommand;

struct TransportProtocol init_tp(enum TransportProtocolMethod protocol, const char *options);

void listen_tp(struct TransportProtocol *tp, void (*func)(struct MessageContainer));

struct MessageContainer create_message(enum MessageType type, enum MessageAction action);

struct PayloadContainer create_message_payload(enum PayloadType type, const char *inner);

void set_message_payload(struct MessageContainer *message, struct PayloadContainer payload);

void set_message_name(struct MessageContainer *message, const char *name);

struct MessageContainer set_state(const char *name, struct PayloadContainer payload);

struct MessageContainer remove_state(const char *name, struct PayloadContainer *payload);

struct MessageContainer emit_signal(const char *name, struct PayloadContainer *payload);

void send_message(const struct TransportProtocol *tp, struct MessageContainer message);

struct InvokeCommand create_invoke(enum InvokeType type, const char *action, const char *payload);

void set_rind_sock_path(char *path);

struct InvokeCommand invoke(struct InvokeCommand command);
