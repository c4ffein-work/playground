<template>
  <div class="chat-assistant">
    <!-- Chat panel -->
    <Transition name="chat-panel">
      <div v-if="open" class="chat-panel floating-panel">
        <div class="chat-header">
          <div class="chat-title">Assistant</div>
          <div class="chat-header-actions">
            <button class="chat-icon-btn" aria-label="Settings" @click="toggleSettings">⚙</button>
            <button class="chat-icon-btn" aria-label="Close" @click="open = false">×</button>
          </div>
        </div>

        <!-- Settings popover -->
        <div v-if="settingsOpen" class="chat-settings">
          <label class="chat-field">
            <span>Anthropic API key</span>
            <input
              type="password"
              v-model="apiKey"
              placeholder="sk-ant-..."
              autocomplete="off"
            >
          </label>
          <label class="chat-field">
            <span>Model</span>
            <input type="text" v-model="model" :placeholder="defaultModel">
          </label>
          <label class="chat-field">
            <span>Base URL (optional proxy)</span>
            <input type="text" v-model="baseUrl" :placeholder="defaultBaseUrl">
          </label>
          <div class="chat-settings-actions">
            <button class="chat-btn" @click="saveSettings">Save</button>
          </div>
        </div>

        <div class="chat-messages" ref="messagesEl">
          <div v-if="!hasKey" class="chat-empty">
            Add your Anthropic API key in settings (⚙) to start chatting.
          </div>
          <div v-else-if="messages.length === 0" class="chat-empty">
            Ask me to inspect or build your Keras model.
          </div>
          <div
            v-for="(message, index) in messages"
            :key="index"
            :class="['chat-message', 'chat-' + message.role]"
          >
            <div v-if="message.role === 'tool'" class="chat-tool">
              <span class="chat-tool-name">{{ message.text }}</span>
            </div>
            <div v-else class="chat-bubble-text">{{ message.text }}</div>
          </div>
          <div v-if="sending" class="chat-message chat-assistant-msg">
            <div class="chat-bubble-text chat-typing">…</div>
          </div>
        </div>

        <form class="chat-input-row" @submit.prevent="send">
          <input
            v-model="draft"
            class="chat-input"
            placeholder="Message the assistant"
            :disabled="sending || !hasKey"
          >
          <button
            type="submit"
            class="chat-send"
            :disabled="sending || !hasKey || draft.trim() === ''"
          >
            Send
          </button>
        </form>
      </div>
    </Transition>

    <!-- Bubble toggle -->
    <button class="chat-fab" aria-label="Toggle assistant" @click="open = !open">
      <span v-if="!open">💬</span>
      <span v-else>×</span>
    </button>
  </div>
</template>

<script>
import AssistantActions from '../../lib/Assistant/assistantActions';
import AnthropicClient, {
  STORAGE_KEY,
  STORAGE_BASE_URL,
  STORAGE_MODEL,
  DEFAULT_MODEL,
  DEFAULT_BASE_URL,
} from '../../lib/Assistant/anthropicClient';

export default {
  name: 'ChatBubble',
  data() {
    return {
      open: false,
      settingsOpen: false,
      draft: '',
      sending: false,
      messages: [],
      history: [],
      apiKey: '',
      model: '',
      baseUrl: '',
      defaultModel: DEFAULT_MODEL,
      defaultBaseUrl: DEFAULT_BASE_URL,
      hasKey: false,
    };
  },
  created() {
    if (typeof localStorage !== 'undefined') {
      this.apiKey = localStorage.getItem(STORAGE_KEY) || '';
      this.model = localStorage.getItem(STORAGE_MODEL) || '';
      this.baseUrl = localStorage.getItem(STORAGE_BASE_URL) || '';
    }
    this.hasKey = Boolean(this.apiKey);
    this.actions = new AssistantActions(this.$d3Interface, this.$kerasInterface);
    this.client = new AnthropicClient(this.actions);
  },
  methods: {
    toggleSettings() {
      this.settingsOpen = !this.settingsOpen;
    },
    saveSettings() {
      if (typeof localStorage !== 'undefined') {
        if (this.apiKey) localStorage.setItem(STORAGE_KEY, this.apiKey);
        else localStorage.removeItem(STORAGE_KEY);
        if (this.model) localStorage.setItem(STORAGE_MODEL, this.model);
        else localStorage.removeItem(STORAGE_MODEL);
        if (this.baseUrl) localStorage.setItem(STORAGE_BASE_URL, this.baseUrl);
        else localStorage.removeItem(STORAGE_BASE_URL);
      }
      this.hasKey = Boolean(this.apiKey);
      this.settingsOpen = false;
    },
    scrollToBottom() {
      this.$nextTick(() => {
        const el = this.$refs.messagesEl;
        if (el) el.scrollTop = el.scrollHeight;
      });
    },
    pushMessage(role, text) {
      this.messages.push({ role, text });
      this.scrollToBottom();
    },
    async send() {
      const text = this.draft.trim();
      if (text === '' || this.sending || !this.hasKey) return;
      this.draft = '';
      this.pushMessage('user', text);
      this.history.push({ role: 'user', content: text });
      this.sending = true;
      try {
        const onActivity = (event) => {
          if (event.type === 'tool_use') {
            this.pushMessage('tool', `⚙ ${event.name}`);
          }
        };
        const reply = await this.client.send(this.history, onActivity);
        if (reply) this.pushMessage('assistant', reply);
      } catch (error) {
        this.pushMessage('assistant', `Error: ${error.message || error}`);
      } finally {
        this.sending = false;
        this.scrollToBottom();
      }
    },
  },
};
</script>

<style scoped>
.chat-assistant {
  position: absolute;
  right: var(--panel-margin);
  bottom: var(--panel-margin);
  z-index: 200;
  font-family: var(--font-regular);
  font-weight: var(--font-weight-regular);
}

.chat-fab {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 52px;
  height: 52px;
  border-radius: 50%;
  border: var(--border-width) solid var(--panel-border);
  background-color: var(--bg-panel);
  box-shadow: var(--panel-shadow);
  font-size: 22px;
  line-height: 1;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: transform 0.15s ease;
}

.chat-fab:hover {
  transform: translateY(-1px);
}

.chat-panel {
  position: absolute;
  right: 0;
  bottom: 64px;
  width: 340px;
  height: 460px;
  max-height: 70vh;
  display: flex;
  flex-direction: column;
  overflow: visible;
}

.chat-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--panel-border);
  font-weight: var(--font-weight-semibold);
}

.chat-title {
  font-weight: var(--font-weight-semibold);
}

.chat-header-actions {
  display: flex;
  gap: 4px;
}

.chat-icon-btn {
  border: none;
  background: transparent;
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  padding: 4px 6px;
  border-radius: 4px;
}

.chat-icon-btn:hover {
  background-color: #f0f0f0;
}

.chat-settings {
  padding: 12px 14px;
  border-bottom: 1px solid var(--panel-border);
  display: flex;
  flex-direction: column;
  gap: 8px;
  background-color: #fafafa;
}

.chat-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
}

.chat-field input {
  font-size: 13px;
}

.chat-settings-actions {
  display: flex;
  justify-content: flex-end;
}

.chat-btn {
  cursor: pointer;
}

.chat-messages {
  flex: 1;
  overflow-y: auto;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.chat-empty {
  color: #666666;
  font-size: 13px;
  text-align: center;
  margin: auto 0;
}

.chat-message {
  display: flex;
}

.chat-user {
  justify-content: flex-end;
}

.chat-bubble-text {
  max-width: 85%;
  padding: 8px 10px;
  border-radius: 12px;
  font-size: 13px;
  white-space: pre-wrap;
  word-break: break-word;
}

.chat-user .chat-bubble-text {
  background-color: #000000;
  color: #ffffff;
}

.chat-assistant .chat-bubble-text,
.chat-assistant-msg .chat-bubble-text {
  background-color: #f0f0f0;
  color: #000000;
}

.chat-tool {
  font-size: 11px;
  color: #666666;
}

.chat-tool-name {
  font-family: monospace;
}

.chat-typing {
  letter-spacing: 2px;
}

.chat-input-row {
  display: flex;
  gap: 6px;
  padding: 10px 12px;
  border-top: 1px solid var(--panel-border);
}

.chat-input {
  flex: 1;
  font-size: 13px;
}

.chat-send {
  cursor: pointer;
}

.chat-send:disabled {
  opacity: 0.5;
  cursor: default;
}

.chat-panel-enter-active,
.chat-panel-leave-active {
  transition: opacity 0.15s ease, transform 0.15s ease;
}

.chat-panel-enter-from,
.chat-panel-leave-to {
  opacity: 0;
  transform: translateY(8px);
}
</style>
