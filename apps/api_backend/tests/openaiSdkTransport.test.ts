import { describe, expect, test } from 'bun:test';
import OpenAI from 'openai';

// Small transport-contract tests: injected fetch, synthetic data, no network or credentials.
const response = (output: unknown[] = []) => ({
  id: 'resp_test',
  object: 'response',
  created_at: 0,
  status: 'completed',
  model: 'test-model',
  output,
  error: null,
  incomplete_details: null,
});
const message = {
  id: 'msg_test',
  type: 'message',
  role: 'assistant',
  status: 'completed',
  content: [{ type: 'output_text', text: 'Ready', annotations: [] }],
};

describe('OpenAI SDK transport used by Talos', () => {
  test('preserves function calls and tool-result continuation', async () => {
    const requests: Request[] = [];
    const client = new OpenAI({
      apiKey: 'test-only',
      maxRetries: 0,
      fetch: async (input, init) => {
        requests.push(new Request(input, init));
        return Response.json(
          response(
            requests.length === 1
              ? [
                  {
                    type: 'function_call',
                    id: 'fc_test',
                    call_id: 'call_test',
                    name: 'test_tool',
                    arguments: '{"value":1}',
                    status: 'completed',
                  },
                ]
              : [message],
          ),
        );
      },
    });
    const first = await client.responses.create({ model: 'test-model', input: 'Test' });
    expect(first.output[0]).toMatchObject({
      type: 'function_call',
      call_id: 'call_test',
      name: 'test_tool',
      arguments: '{"value":1}',
    });
    const final = await client.responses.create({
      model: 'test-model',
      previous_response_id: first.id,
      input: [{ type: 'function_call_output', call_id: 'call_test', output: 'ok' }],
    });
    expect(final.output_text).toBe('Ready');
    expect(requests).toHaveLength(2);
    expect(requests[0]?.url).toBe('https://api.openai.com/v1/responses');
    expect(requests[0]?.method).toBe('POST');
    expect(await requests[1]?.json()).toMatchObject({
      previous_response_id: 'resp_test',
      input: [{ type: 'function_call_output', call_id: 'call_test', output: 'ok' }],
    });
  });

  test('streams output deltas and exposes the final response', async () => {
    const events = [
      { type: 'response.created', response: { ...response(), status: 'in_progress' } },
      {
        type: 'response.output_item.added',
        output_index: 0,
        item: { ...message, status: 'in_progress', content: [] },
      },
      {
        type: 'response.content_part.added',
        output_index: 0,
        content_index: 0,
        item_id: 'msg_test',
        part: { type: 'output_text', text: '', annotations: [] },
      },
      {
        type: 'response.output_text.delta',
        output_index: 0,
        content_index: 0,
        item_id: 'msg_test',
        delta: 'Ready',
      },
      { type: 'response.completed', response: response([message]) },
    ];
    const client = new OpenAI({
      apiKey: 'test-only',
      maxRetries: 0,
      fetch: async (_input, init) => {
        expect(JSON.parse(String(init?.body))).toMatchObject({ stream: true });
        return new Response(
          events
            .map(
              (event, sequence_number) =>
                `event: ${event.type}\ndata: ${JSON.stringify({ ...event, sequence_number })}\n\n`,
            )
            .join(''),
          { headers: { 'Content-Type': 'text/event-stream' } },
        );
      },
    });
    const stream = client.responses.stream({ model: 'test-model', input: 'Test' });
    let text = '';
    for await (const event of stream) {
      if (event.type === 'response.output_text.delta') text += event.delta;
    }
    expect(text).toBe('Ready');
    expect((await stream.finalResponse()).output_text).toBe('Ready');
  });

  test('honors a cancelled request without sending it', async () => {
    let calls = 0;
    const client = new OpenAI({
      apiKey: 'test-only',
      maxRetries: 0,
      fetch: async () => {
        calls += 1;
        return Response.json(response());
      },
    });
    const controller = new AbortController();
    controller.abort();
    let failure: unknown;
    try {
      await client.responses.create(
        { model: 'test-model', input: 'Test' },
        {
          signal: controller.signal,
        },
      );
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(OpenAI.APIUserAbortError);
    expect(calls).toBe(0);
  });
});
