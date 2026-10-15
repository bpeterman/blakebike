import { Component, type ErrorInfo, type ReactNode } from "react";
import { api } from "./api";

type Props = { children: ReactNode };
type State = { error: Error | null };

/**
 * Last line of defense for the window. A render error anywhere would
 * otherwise blank the whole screen while the ride keeps going in the backend
 * with no way to pause or end it. The fallback keeps those two controls
 * available and offers a reload; the backend and the ride are untouched.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    void api
      .reportError("render", `${error.message}\n${info.componentStack ?? ""}`)
      .catch(() => undefined);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="loading">
        <div className="loading-error" role="alert">
          <h2>The screen hit an error</h2>
          <p>{this.state.error.message}</p>
          <span className="error-boundary-note">
            If a ride is running it continues in the background. You can pause or end it here, then reload.
          </span>
          <div className="error-boundary-actions">
            <button type="button" className="secondary" onClick={() => void api.pauseOrResume().catch(() => undefined)}>
              Pause / Resume
            </button>
            <button type="button" className="stop" onClick={() => void api.stopWorkout().catch(() => undefined)}>
              End ride
            </button>
            <button type="button" className="primary" onClick={() => window.location.reload()}>
              Reload
            </button>
          </div>
        </div>
      </div>
    );
  }
}
