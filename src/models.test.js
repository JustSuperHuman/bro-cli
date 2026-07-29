import { expect, test } from 'bun:test';
import { mapOpenRouterModels } from './models.js';

test('OpenRouter mapping keeps models from every publisher and orders newest first', () => {
  const models = mapOpenRouterModels([
    { id: 'anthropic/claude-sonnet', name: 'Claude Sonnet', created: 20 },
    { id: 'google/gemini-pro', name: 'Gemini Pro', created: 30 },
    { id: 'meta/llama', name: 'Llama', created: 10 },
    { name: 'Missing id', created: 40 }
  ]);

  expect(models).toEqual([
    { id: 'google/gemini-pro', name: 'Gemini Pro' },
    { id: 'anthropic/claude-sonnet', name: 'Claude Sonnet' },
    { id: 'meta/llama', name: 'Llama' }
  ]);
});

test('OpenRouter mapping falls back to the model id when its name is missing', () => {
  expect(mapOpenRouterModels([{ id: 'new/model', created: 1 }])).toEqual([
    { id: 'new/model', name: 'new/model' }
  ]);
});
