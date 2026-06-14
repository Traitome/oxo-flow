/// SSE EventSource wrapper with last-event-id reconnection support.
export class SSEClient {
  private es: EventSource | null = null;
  private url: string;
  private lastEventId: string | null = null;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;

  constructor(url: string) {
    this.url = url;
  }

  connect(onMessage: (event: string, data: any) => void, onError?: (e: Event) => void) {
    if (this.es) this.es.close();
    const connectUrl = this.lastEventId
      ? `${this.url}?lastEventId=${encodeURIComponent(this.lastEventId)}`
      : this.url;
    this.es = new EventSource(connectUrl);
    this.es.onopen = () => { this.reconnectAttempts = 0; };
    this.es.onmessage = (event) => {
      this.lastEventId = event.lastEventId || null;
      try { const d = JSON.parse(event.data); onMessage(event.type || 'message', d); }
      catch { onMessage(event.type || 'message', event.data); }
    };
    this.es.onerror = (error) => {
      onError?.(error);
      if (this.reconnectAttempts < this.maxReconnectAttempts) {
        this.reconnectAttempts++;
        setTimeout(() => this.connect(onMessage, onError), Math.min(1000 * Math.pow(2, this.reconnectAttempts), 30000));
      }
    };
  }

  disconnect() { this.es?.close(); this.es = null; }
  isConnected(): boolean { return this.es?.readyState === EventSource.OPEN; }
}
