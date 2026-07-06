import { describe, it, expect } from 'vitest';
import KerasGenerator from '../../src/lib/KerasInterface/KerasGenerator';
import KerasGeneratorPyTorchHelper from '../../src/lib/KerasInterface/KerasGeneratorPyTorchHelper';

// --- Fixture builders (mirror KerasGenerator.test.js) -----------------------
// KerasGenerator mutates the layer objects it is given, so every fixture MUST be
// produced fresh from a function and never shared between two instances.

function leaf(id, name, {
  params = {}, def = {}, inputLayers = [], outputLayers = [],
} = {}) {
  return {
    id,
    x: 0,
    y: 0,
    name,
    inputLayers,
    outputLayers,
    children: null,
    kerasLayer: {
      name, category: 'test', parameterValues: params, parameterDef: def,
    },
  };
}

// Sequential: Input(1) -> Flatten(2) -> Dense(3,128) -> Dense(4,10) -> Output(5)
function sequentialJson() {
  return {
    inputs: ['1'],
    outputs: ['5'],
    layers: [
      leaf('1', 'Input', { params: { shape: [28, 28] }, outputLayers: ['2'] }),
      leaf('2', 'Flatten', { inputLayers: ['1'], outputLayers: ['3'] }),
      leaf('3', 'Dense', { params: { units: 128 }, inputLayers: ['2'], outputLayers: ['4'] }),
      leaf('4', 'Dense', { params: { units: 10 }, inputLayers: ['3'], outputLayers: ['5'] }),
      leaf('5', 'Output', { inputLayers: ['4'] }),
    ],
  };
}

// Branching functional:
// Input(1) -> Dense(2,4) -\
//          -> Dense(3,4) --> Concatenate(4) -> Output(5)
function functionalJson() {
  return {
    inputs: ['1'],
    outputs: ['5'],
    layers: [
      leaf('1', 'Input', { params: { shape: [10] }, outputLayers: ['2', '3'] }),
      leaf('2', 'Dense', { params: { units: 4 }, inputLayers: ['1'], outputLayers: ['4'] }),
      leaf('3', 'Dense', { params: { units: 4 }, inputLayers: ['1'], outputLayers: ['4'] }),
      leaf('4', 'Concatenate', { inputLayers: ['2', '3'], outputLayers: ['5'] }),
      leaf('5', 'Output', { inputLayers: ['4'] }),
    ],
  };
}

// A layer NNVP does not know how to map to torch.nn -> TODO placeholder.
// Input(1) -> LSTM(2) -> Output(3) (still a linear chain -> sequential path).
function unsupportedJson() {
  return {
    inputs: ['1'],
    outputs: ['3'],
    layers: [
      leaf('1', 'Input', { params: { shape: [10] }, outputLayers: ['2'] }),
      leaf('2', 'LSTM', { params: { units: 8 }, inputLayers: ['1'], outputLayers: ['3'] }),
      leaf('3', 'Output', { inputLayers: ['2'] }),
    ],
  };
}

// ---------------------------------------------------------------------------

describe('PyTorch: full generation', () => {
  it('generates a nn.Module subclass for a sequential chain', () => {
    const code = new KerasGenerator(sequentialJson()).generatePyTorchFromGraph();
    expect(code).toBe(
      'import torch\n'
      + 'import torch.nn as nn\n'
      + '\n'
      + '\n'
      + 'class Model(nn.Module):\n'
      + '  def __init__(self):\n'
      + '    super().__init__()\n'
      + '    self.layer_2 = nn.Flatten()\n'
      + '    self.layer_3 = nn.LazyLinear(128)\n'
      + '    self.layer_4 = nn.LazyLinear(10)\n'
      + '\n'
      + '  def forward(self, x):\n'
      + '    x = self.layer_2(x)\n'
      + '    x = self.layer_3(x)\n'
      + '    x = self.layer_4(x)\n'
      + '    return x\n',
    );
  });

  it('wires a branching functional graph through forward() dataflow', () => {
    const code = new KerasGenerator(functionalJson()).generatePyTorchFromGraph();
    expect(code).toBe(
      'import torch\n'
      + 'import torch.nn as nn\n'
      + '\n'
      + '\n'
      + 'class Model(nn.Module):\n'
      + '  def __init__(self):\n'
      + '    super().__init__()\n'
      + '    self.layer_2 = nn.LazyLinear(4)\n'
      + '    self.layer_3 = nn.LazyLinear(4)\n'
      + '\n'
      + '  def forward(self, x):\n'
      + '    input_1 = x\n'
      + '    layer_2 = self.layer_2(input_1)\n'
      + '    layer_3 = self.layer_3(input_1)\n'
      + '    layer_4 = torch.cat([layer_2, layer_3], dim=1)\n'
      + '    return layer_4\n',
    );
  });

  it('emits a clearly-marked TODO placeholder for an unsupported layer', () => {
    const code = new KerasGenerator(unsupportedJson()).generatePyTorchFromGraph();
    expect(code).toBe(
      'import torch\n'
      + 'import torch.nn as nn\n'
      + '\n'
      + '\n'
      + 'class Model(nn.Module):\n'
      + '  def __init__(self):\n'
      + '    super().__init__()\n'
      + '    # TODO: unsupported layer LSTM\n'
      + '\n'
      + '  def forward(self, x):\n'
      + '    x = x  # TODO: unsupported layer LSTM\n'
      + '    return x\n',
    );
    // The placeholder must not silently emit wrong torch code.
    expect(code).not.toContain('nn.LSTM');
  });
});

describe('PyTorch: layer -> torch.nn mapping', () => {
  const ctor = (name, params = {}) => {
    const graph = { n: { keras_data: { name, parameterValues: params } } };
    return new KerasGeneratorPyTorchHelper(graph, [], [], [], false).moduleConstructor('n');
  };

  it('maps the common layers to lazy modules where shape is unknown', () => {
    expect(ctor('Dense', { units: 32 })).toBe('nn.LazyLinear(32)');
    expect(ctor('Conv2D', { filters: 16, kernel_size: [3, 3] })).toBe('nn.LazyConv2d(16, (3,3,))');
    expect(ctor('Conv1D', { filters: 8, kernel_size: 5 })).toBe('nn.LazyConv1d(8, 5)');
    expect(ctor('Flatten')).toBe('nn.Flatten()');
    expect(ctor('MaxPooling2D', { pool_size: [2, 2] })).toBe('nn.MaxPool2d((2,2,))');
    expect(ctor('AveragePooling2D', { pool_size: [2, 2] })).toBe('nn.AvgPool2d((2,2,))');
    expect(ctor('Dropout', { rate: 0.25 })).toBe('nn.Dropout(0.25)');
    expect(ctor('BatchNormalization')).toBe('nn.LazyBatchNorm1d()');
  });

  it('maps activations to matching nn modules', () => {
    expect(ctor('Activation', { activation: 'relu' })).toBe('nn.ReLU()');
    expect(ctor('Activation', { activation: 'softmax' })).toBe('nn.Softmax(dim=1)');
    expect(ctor('Sigmoid')).toBe('nn.Sigmoid()');
  });

  it('returns null (-> TODO) for an unmapped layer or unknown activation', () => {
    expect(ctor('LSTM', { units: 8 })).toBeNull();
    expect(ctor('Activation', { activation: 'mish' })).toBeNull();
  });
});
