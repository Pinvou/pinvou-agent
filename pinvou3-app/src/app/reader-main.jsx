import React from 'react';
import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import { ReaderApp } from '../features/reader/ReaderApp.jsx';

createRoot(document.getElementById('root')).render(<ReaderApp />);
