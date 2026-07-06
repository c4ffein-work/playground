// Browser-side Anthropic Messages API client with a tool-use loop.
//
// A static NNVP SPA can't hide a key, so this is bring-your-own-key: the API key
// (and an optional custom base URL) live in localStorage and are read at call
// time. Each AssistantActions method is exposed as a tool; the loop sends the
// conversation, executes any tool_use blocks against the actions, feeds back
// tool_result blocks, and repeats until the model returns a final text answer.

export const STORAGE_KEY = 'nnvp_anthropic_key';
export const STORAGE_BASE_URL = 'nnvp_anthropic_base_url';
export const STORAGE_MODEL = 'nnvp_anthropic_model';

export const DEFAULT_MODEL = 'claude-sonnet-5';
export const DEFAULT_BASE_URL = 'https://api.anthropic.com';
export const ANTHROPIC_VERSION = '2023-06-01';

export const SYSTEM_PROMPT = [
  'You are the NNVP assistant, embedded in a visual editor that builds Keras models.',
  'You can inspect and modify the model graph through the provided tools.',
  'Prefer calling a tool over guessing. When you add layers or change parameters,',
  'briefly confirm what you did. Keep answers concise.',
].join(' ');

// Tool surface: one entry per AssistantActions method the model may call.
// buildTools() returns the array sent to the API; TOOL_DISPATCH maps each tool
// name to how it invokes the actions instance.
export function buildTools() {
  return [
    {
      name: 'list_layer_types',
      description: 'List every available Keras layer type name that can be added.',
      input_schema: { type: 'object', properties: {} },
    },
    {
      name: 'add_layer',
      description: 'Add a new layer of the given type to the model graph.',
      input_schema: {
        type: 'object',
        properties: {
          type_name: { type: 'string', description: 'A layer type from list_layer_types.' },
        },
        required: ['type_name'],
      },
    },
    {
      name: 'list_layers',
      description: 'List the current layers with their id, type, name and parameter values.',
      input_schema: { type: 'object', properties: {} },
    },
    {
      name: 'set_param',
      description: 'Set a parameter value on a layer, identified by its id.',
      input_schema: {
        type: 'object',
        properties: {
          layer_id: { description: 'The id of the layer to modify.' },
          param_name: { type: 'string', description: 'The parameter name.' },
          value: { description: 'The new parameter value (any JSON value).' },
        },
        required: ['layer_id', 'param_name', 'value'],
      },
    },
    {
      name: 'get_model_summary',
      description: 'Get counts of layers/inputs/outputs/edges plus a compact layer list.',
      input_schema: { type: 'object', properties: {} },
    },
    {
      name: 'generate_code',
      description: 'Generate the Keras model source code and return it as a string.',
      input_schema: {
        type: 'object',
        properties: {
          lang: { type: 'string', enum: ['python', 'javascript'] },
        },
        required: ['lang'],
      },
    },
    {
      name: 'delete_selected',
      description: 'Delete the currently selected layers/edges from the graph.',
      input_schema: { type: 'object', properties: {} },
    },
    {
      name: 'undo',
      description: 'Undo the last graph change.',
      input_schema: { type: 'object', properties: {} },
    },
    {
      name: 'redo',
      description: 'Redo the last undone graph change.',
      input_schema: { type: 'object', properties: {} },
    },
  ];
}

const TOOL_DISPATCH = {
  list_layer_types: (actions) => actions.listLayerTypes(),
  add_layer: (actions, input) => actions.addLayer(input.type_name),
  list_layers: (actions) => actions.listLayers(),
  set_param: (actions, input) => actions.setParam(input.layer_id, input.param_name, input.value),
  get_model_summary: (actions) => actions.getModelSummary(),
  generate_code: (actions, input) => actions.generateCode(input.lang),
  delete_selected: (actions) => actions.deleteSelected(),
  undo: (actions) => actions.undo(),
  redo: (actions) => actions.redo(),
};

// Read config from localStorage when available, letting explicit options win.
export function readStoredConfig(overrides = {}) {
  let stored = {};
  if (typeof localStorage !== 'undefined') {
    stored = {
      apiKey: localStorage.getItem(STORAGE_KEY) || '',
      baseUrl: localStorage.getItem(STORAGE_BASE_URL) || '',
      model: localStorage.getItem(STORAGE_MODEL) || '',
    };
  }
  return {
    apiKey: overrides.apiKey || stored.apiKey || '',
    baseUrl: overrides.baseUrl || stored.baseUrl || DEFAULT_BASE_URL,
    model: overrides.model || stored.model || DEFAULT_MODEL,
  };
}

export default class AnthropicClient {
  constructor(actions, options = {}) {
    this.actions = actions;
    this.options = options;
    // Allow tests to inject a fetch implementation.
    this.fetchImpl = options.fetch || (typeof fetch !== 'undefined' ? fetch.bind(globalThis) : null);
    this.maxTurns = options.maxTurns || 12;
  }

  config() {
    return readStoredConfig(this.options);
  }

  hasApiKey() {
    return Boolean(this.config().apiKey);
  }

  // The tools array sent to the API, one per exposed action.
  tools() {
    return buildTools();
  }

  // Execute a single tool call against the actions, returning the JSON-string
  // content and an is_error flag for the tool_result block.
  runTool(name, input) {
    const handler = TOOL_DISPATCH[name];
    if (handler === undefined) {
      return { content: `Unknown tool "${name}".`, isError: true };
    }
    try {
      const result = handler(this.actions, input || {});
      const content = typeof result === 'string' ? result : JSON.stringify(result);
      return { content, isError: false };
    } catch (error) {
      return { content: error.message || String(error), isError: true };
    }
  }

  async postMessages(messages) {
    const { apiKey, baseUrl, model } = this.config();
    if (!apiKey) {
      throw new Error('No Anthropic API key set. Add one in the assistant settings.');
    }
    if (!this.fetchImpl) {
      throw new Error('No fetch implementation available.');
    }
    const response = await this.fetchImpl(`${baseUrl}/v1/messages`, {
      method: 'POST',
      headers: {
        'x-api-key': apiKey,
        'anthropic-version': ANTHROPIC_VERSION,
        'anthropic-dangerous-direct-browser-access': 'true',
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        model,
        max_tokens: this.options.maxTokens || 2048,
        system: SYSTEM_PROMPT,
        messages,
        tools: this.tools(),
      }),
    });
    if (!response.ok) {
      let detail = '';
      try {
        detail = await response.text();
      } catch { /* ignore */ }
      throw new Error(`Anthropic API error ${response.status}: ${detail}`);
    }
    return response.json();
  }

  // Run the tool-use loop for one user turn. `history` is the running list of
  // Anthropic messages; it is mutated in place with the new turns. `onActivity`
  // (optional) is called with { type, ... } as tools run, for UI feedback.
  // Returns the final assistant text.
  async send(history, onActivity) {
    const notify = onActivity || (() => {});
    for (let turn = 0; turn < this.maxTurns; turn += 1) {
      const reply = await this.postMessages(history);
      history.push({ role: 'assistant', content: reply.content });

      const toolUses = reply.content.filter(block => block.type === 'tool_use');
      const texts = reply.content
        .filter(block => block.type === 'text')
        .map(block => block.text)
        .join('\n');

      if (toolUses.length === 0) {
        return texts;
      }
      if (texts) notify({ type: 'text', text: texts });

      const toolResults = toolUses.map(block => {
        notify({ type: 'tool_use', name: block.name, input: block.input });
        const { content, isError } = this.runTool(block.name, block.input);
        notify({ type: 'tool_result', name: block.name, content, isError });
        return {
          type: 'tool_result',
          tool_use_id: block.id,
          content,
          is_error: isError,
        };
      });
      history.push({ role: 'user', content: toolResults });
    }
    throw new Error('Assistant stopped after too many tool-use turns.');
  }
}
