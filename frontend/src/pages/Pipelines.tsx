import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { Template } from '../api/types';

export default function Pipelines() {
  const [templates, setTemplates] = useState<Template[]>([]);

  useEffect(() => {
    api.listTemplates().then(setTemplates).catch(() => {});
  }, []);

  const categories = [...new Set(templates.map((t) => t.category))];

  return (
    <div className="page">
      <h1 className="page-title">Pipeline Library</h1>

      {categories.length === 0 ? (
        <div className="empty-state">No templates available.</div>
      ) : (
        categories.map((cat) => (
          <div key={cat} className="section">
            <h2 className="section-title">{cat}</h2>
            <div className="template-grid">
              {templates
                .filter((t) => t.category === cat)
                .map((t) => (
                  <div key={t.id} className="template-card">
                    <h3>{t.name}</h3>
                    <p>{t.description}</p>
                    <div className="template-meta">
                      <span className="tag">{t.tags}</span>
                    </div>
                    <Link to="/editor" className="template-use">Use Template</Link>
                  </div>
                ))}
            </div>
          </div>
        ))
      )}
    </div>
  );
}
