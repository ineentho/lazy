import React from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

createRoot(document.getElementById("root")).render(
  <main>
    <p className="eyebrow">lazy + vite</p>
    <h1>Hello from Vite</h1>
    <p>This dev server was started by the first proxied request.</p>
  </main>
);
