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

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef enum RIND_TP_METHOD {
  RIND_TP_METHOD_STDIO = 0,
  RIND_TP_METHOD_UDS = 1,
  RIND_TP_METHOD_SHM = 2,
} RIND_TP_METHOD;

typedef enum RIND_MSG_TYPE {
  RIND_MSG_TYPE_IMPULSE = 0,
  RIND_MSG_TYPE_FACET = 1,
  RIND_MSG_TYPE_ENQUIRY = 2,
  RIND_MSG_TYPE_RESPONSE = 3,
  RIND_MSG_TYPE_UNKNOWN = 4,
} RIND_MSG_TYPE;

typedef enum RIND_MSG_ACTION {
  RIND_MSG_ACTION_REMOVE = 0,
  RIND_MSG_ACTION_SET = 1,
} RIND_MSG_ACTION;

typedef enum RIND_PAYLOAD_TYPE {
  RIND_PAYLOAD_TYPE_STRING = 0,
  RIND_PAYLOAD_TYPE_JSON = 1,
} RIND_PAYLOAD_TYPE;

typedef enum RIND_INVOKE_TYPE {
  RIND_INVOKE_TYPE_VALID = 0,
  RIND_INVOKE_TYPE_OK = 1,
  RIND_INVOKE_TYPE_ERROR = 2,
  RIND_INVOKE_TYPE_UNKNOWN = 3,
  RIND_INVOKE_TYPE_REQUEST_INPUT = 4,
  RIND_INVOKE_TYPE_ENQUIRE = 5,
} RIND_INVOKE_TYPE;

typedef struct rind_tp {
  enum RIND_TP_METHOD protocol;
  const char *const *options;
  uintptr_t len;
  uint64_t id;
} rind_tp;

typedef struct rind_payload {
  enum RIND_PAYLOAD_TYPE type;
  const char *content;
} rind_payload;

typedef struct rind_msg {
  enum RIND_MSG_TYPE type;
  enum RIND_MSG_ACTION action;
  struct rind_payload *payload;
  const char *name;
} rind_msg;

typedef struct rind_invoke_cmd {
  enum RIND_INVOKE_TYPE type;
  const char *action;
  const char *payload;
} rind_invoke_cmd;

struct rind_tp rind_init_tp(enum RIND_TP_METHOD protocol, const char *options);

void rind_listen_tp(struct rind_tp *tp, void (*func)(struct rind_msg));

struct rind_msg rind_enquiry_tp(const struct rind_tp *tp, struct rind_msg message);

struct rind_msg rind_create_msg(enum RIND_MSG_TYPE type, enum RIND_MSG_ACTION action);

struct rind_payload rind_create_msg_payload(enum RIND_PAYLOAD_TYPE type, const char *inner);

void rind_set_message_payload(struct rind_msg *message, struct rind_payload payload);

void rind_set_message_name(struct rind_msg *message, const char *name);

struct rind_msg rind_set_facet(const char *name, struct rind_payload payload);

struct rind_msg rind_remove_facet(const char *name, struct rind_payload *payload);

struct rind_msg rind_impulse(const char *name, struct rind_payload *payload);

struct rind_msg rind_log_msg(const char *log);

uint8_t rind_send_message(const struct rind_tp *tp, struct rind_msg message);

struct rind_invoke_cmd rind_create_invoke(enum RIND_INVOKE_TYPE type,
                                          const char *action,
                                          const char *payload);

void rind_set_sock_path(char *path);

struct rind_invoke_cmd rind_invoke(struct rind_invoke_cmd command);
