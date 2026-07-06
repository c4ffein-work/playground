import { describe, it, expect, beforeEach } from 'vitest';
import AssistantActions from '../../src/lib/Assistant/assistantActions';
import AnthropicClient, { buildTools } from '../../src/lib/Assistant/anthropicClient';

// --- Fakes ------------------------------------------------------------------
// A minimal stand-in for a KerasLayer: name + parameterValues + setter + clone,
// matching the surface AssistantActions relies on.
function makeKerasLayer(name) {
  return {
    name,
    parameterValues: {},
    setParameterValue(paramName, value) {
      this.parameterValues[paramName] = value;
    },
    clone() {
      const copy = makeKerasLayer(this.name);
      copy.parameterValues = JSON.parse(JSON.stringify(this.parameterValues));
      return copy;
    },
  };
}

// A fake $d3Interface mirroring the real add/find/model structure closely
// enough to exercise the actions (activeGraph.model.d3Layers, findLayerById...).
function makeFakeD3Interface() {
  const model = {
    d3Layers: [],
    d3Edges: [],
    modelInputs: [],
    modelOutputs: [],
  };
  let nextId = 1;
  const activeGraph = {
    model,
    toJSON() {
      return JSON.stringify({ layers: model.d3Layers.map(l => l.id) });
    },
    findLayerById(id) {
      return model.d3Layers.find(layer => layer.id === id) || null;
    },
  };
  return {
    activeGraph,
    calls: { deleteSelected: 0, undo: 0, redo: 0 },
    addLayer(kerasLayer) {
      const id = nextId;
      nextId += 1;
      model.d3Layers.push({ id, name: kerasLayer.name, kerasLayer });
    },
    findLayerById(id) {
      return activeGraph.findLayerById(id);
    },
    deleteSelectedElements() {
      this.calls.deleteSelected += 1;
    },
    undo() {
      this.calls.undo += 1;
    },
    redo() {
      this.calls.redo += 1;
    },
  };
}

function makeFakeKerasInterface() {
  const layerList = {
    Dense: makeKerasLayer('Dense'),
    Input: makeKerasLayer('Input'),
  };
  return {
    getLayerList() {
      return layerList;
    },
    generatePython(json) {
      return `# python for ${json}`;
    },
    generateJavascript(json) {
      return `// javascript for ${json}`;
    },
  };
}

// --- Tests ------------------------------------------------------------------
describe('AssistantActions', () => {
  let actions;
  let d3;
  let keras;

  beforeEach(() => {
    d3 = makeFakeD3Interface();
    keras = makeFakeKerasInterface();
    actions = new AssistantActions(d3, keras);
  });

  it('lists available layer types', () => {
    expect(actions.listLayerTypes()).toEqual(['Dense', 'Input']);
  });

  it('adds a layer and lists it', () => {
    const added = actions.addLayer('Dense');
    expect(added.type).toBe('Dense');
    expect(added.id).not.toBeNull();

    const layers = actions.listLayers();
    expect(layers).toHaveLength(1);
    expect(layers[0].type).toBe('Dense');
    expect(layers[0].id).toBe(added.id);
    expect(layers[0].params).toEqual({});
  });

  it('rejects an unknown layer type', () => {
    expect(() => actions.addLayer('NotALayer')).toThrow(/Unknown layer type/);
  });

  it('sets a parameter value on a layer', () => {
    const added = actions.addLayer('Dense');
    const result = actions.setParam(added.id, 'units', 64);
    expect(result.params.units).toBe(64);

    const layers = actions.listLayers();
    expect(layers[0].params.units).toBe(64);
  });

  it('throws when setting a param on a missing layer', () => {
    expect(() => actions.setParam(999, 'units', 1)).toThrow(/No layer with id/);
  });

  it('summarizes the model with counts and a compact layer list', () => {
    actions.addLayer('Input');
    actions.addLayer('Dense');
    d3.activeGraph.model.modelInputs.push(d3.activeGraph.model.d3Layers[0]);
    d3.activeGraph.model.d3Edges.push({});

    const summary = actions.getModelSummary();
    expect(summary.layerCount).toBe(2);
    expect(summary.inputCount).toBe(1);
    expect(summary.outputCount).toBe(0);
    expect(summary.edgeCount).toBe(1);
    expect(summary.layers.map(l => l.type)).toEqual(['Input', 'Dense']);
  });

  it('generates python and javascript code', () => {
    actions.addLayer('Dense');
    expect(actions.generateCode('python')).toContain('# python for');
    expect(actions.generateCode('javascript')).toContain('// javascript for');
    expect(() => actions.generateCode('ruby')).toThrow(/Unknown language/);
  });

  it('delegates delete/undo/redo to the d3 interface', () => {
    actions.deleteSelected();
    actions.undo();
    actions.redo();
    expect(d3.calls).toEqual({ deleteSelected: 1, undo: 1, redo: 1 });
  });

  it('reports a friendly error when no graph is active', () => {
    const bare = new AssistantActions({ activeGraph: null }, keras);
    expect(() => bare.listLayers()).toThrow(/No active graph/);
  });
});

describe('AnthropicClient tool mapping', () => {
  it('builds a valid tools array from the actions', () => {
    const tools = buildTools();
    const names = tools.map(t => t.name);
    expect(names).toEqual([
      'list_layer_types',
      'add_layer',
      'list_layers',
      'set_param',
      'get_model_summary',
      'generate_code',
      'delete_selected',
      'undo',
      'redo',
    ]);
    tools.forEach((tool) => {
      expect(typeof tool.name).toBe('string');
      expect(typeof tool.description).toBe('string');
      expect(tool.input_schema).toBeDefined();
      expect(tool.input_schema.type).toBe('object');
    });
  });

  it('executes a mapped tool against the actions', () => {
    const d3 = makeFakeD3Interface();
    const keras = makeFakeKerasInterface();
    const actions = new AssistantActions(d3, keras);
    const client = new AnthropicClient(actions);

    const listed = client.runTool('list_layer_types', {});
    expect(listed.isError).toBe(false);
    expect(JSON.parse(listed.content)).toEqual(['Dense', 'Input']);

    const added = client.runTool('add_layer', { type_name: 'Dense' });
    expect(added.isError).toBe(false);

    const bad = client.runTool('add_layer', { type_name: 'Nope' });
    expect(bad.isError).toBe(true);

    const unknown = client.runTool('does_not_exist', {});
    expect(unknown.isError).toBe(true);
  });
});
