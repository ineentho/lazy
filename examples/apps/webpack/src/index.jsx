import React from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

createRoot(document.getElementById("root")).render(
  <main>
    <h1>Hello from Webpack</h1>
    <p>Webpack dev server is reading its port from the lazy runner.</p>
  </main>
);
