import struct
import lzma
import sys
import os
import json
import numpy as np

def read_uleb128(f):
    result = 0
    shift = 0
    while True:
        byte = f.read(1)[0]
        result |= (byte & 0x7f) << shift
        if not (byte & 0x80):
            break
        shift += 7
    return result

def read_osu_string(f):
    indicator = f.read(1)[0]
    if indicator == 0x00:
        return ""
    elif indicator == 0x0b:
        length = read_uleb128(f)
        return f.read(length).decode('utf-8', errors='replace')
    else:
        raise ValueError(f"Invalid string indicator: {indicator}")

def parse_osr(file_path):
    with open(file_path, 'rb') as f:
        mode = f.read(1)[0]
        version = struct.unpack('<i', f.read(4))[0]
        map_md5 = read_osu_string(f)
        player_name = read_osu_string(f)
        replay_md5 = read_osu_string(f)
        
        c300, c100, c50, c_geki, c_katu, c_miss = struct.unpack('<hhhhhh', f.read(12))
        score, max_combo = struct.unpack('<ih', f.read(6))
        perfect = f.read(1)[0]
        mods = struct.unpack('<i', f.read(4))[0]
        life_graph = read_osu_string(f)
        timestamp = struct.unpack('<q', f.read(8))[0]
        
        lzma_len = struct.unpack('<i', f.read(4))[0]
        lzma_data = f.read(lzma_len)
        score_id = struct.unpack('<q', f.read(8))[0] if f.tell() + 8 <= os.path.getsize(file_path) else 0

    decompressed = lzma.decompress(lzma_data).decode('ascii', errors='replace')
    frames = []
    current_time = 0
    for part in decompressed.split(','):
        if not part or '|' not in part:
            continue
        tokens = part.split('|')
        if len(tokens) >= 4:
            w = float(tokens[0])
            if w == -12345: # RNG seed at end of stream
                continue
            x = float(tokens[1])
            y = float(tokens[2])
            z = int(tokens[3])
            current_time += w
            frames.append({
                'time_ms': current_time,
                'delta_ms': w,
                'x': x,
                'y': y,
                'keys': z
            })

    return {
        'mode': mode,
        'version': version,
        'map_md5': map_md5,
        'player_name': player_name,
        'replay_md5': replay_md5,
        'count_300': c300,
        'count_100': c100,
        'count_50': c50,
        'count_geki': c_geki,
        'count_katu': c_katu,
        'count_miss': c_miss,
        'score': score,
        'max_combo': max_combo,
        'perfect': perfect,
        'mods': mods,
        'timestamp': timestamp,
        'score_id': score_id,
        'frames': frames
    }

def parse_osu_hitobjects(osu_path):
    objects = []
    in_objects = False
    with open(osu_path, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if line == '[HitObjects]':
                in_objects = True
                continue
            if in_objects:
                if not line or line.startswith('['):
                    break
                parts = line.split(',')
                if len(parts) >= 5:
                    x = float(parts[0])
                    y = float(parts[1])
                    t = float(parts[2])
                    obj_type = int(parts[3])
                    is_circle = bool(obj_type & 1)
                    is_slider = bool(obj_type & 2)
                    is_spinner = bool(obj_type & 8)
                    objects.append({
                        'x': x,
                        'y': y,
                        'time_ms': t,
                        'is_circle': is_circle,
                        'is_slider': is_slider,
                        'is_spinner': is_spinner
                    })
    return objects

def analyze_replay_against_beatmap(osr_path, osu_path, od=3.0, cs=3.0):
    replay = parse_osr(osr_path)
    objects = parse_osu_hitobjects(osu_path)
    
    # Lazer hit windows for OD 3.0:
    # 300: 80 - 6 * OD = 62 ms (or stable: 80 - 6 * 3 = 62 ms)
    # 100: 140 - 8 * OD = 116 ms
    # 50:  200 - 10 * OD = 170 ms
    # Circle radius = 54.4 - 4.48 * CS = 40.96 px
    w300 = 80.0 - 6.0 * od
    w100 = 140.0 - 8.0 * od
    w50 = 200.0 - 10.0 * od
    radius = 54.4 - 4.48 * cs

    # Find key presses (transitions where keys went from 0 to >0 or bit change)
    key_presses = []
    prev_keys = 0
    for f in replay['frames']:
        k = f['keys']
        # If any key pressed that was not pressed before
        # Key bits: 1=M1, 2=M2, 4=K1, 8=K2
        new_press = (k & ~prev_keys) & 15
        if new_press > 0:
            key_presses.append(f)
        prev_keys = k

    # Match each object to closest press
    judgements = []
    hit_errors = []
    hit_times = []

    for obj in objects:
        t_obj = obj['time_ms']
        # Find presses within w50
        candidates = [p for p in key_presses if abs(p['time_ms'] - t_obj) <= w50]
        if not candidates:
            # Fallback: find closest press
            closest = min(key_presses, key=lambda p: abs(p['time_ms'] - t_obj), default=None)
            if closest and abs(closest['time_ms'] - t_obj) <= w50:
                candidates = [closest]

        if candidates:
            best = min(candidates, key=lambda p: abs(p['time_ms'] - t_obj))
            dt = best['time_ms'] - t_obj
            dist = np.hypot(best['x'] - obj['x'], best['y'] - obj['y'])
            
            # Check spatial validity
            valid_space = True if obj['is_spinner'] else (dist <= radius)
            
            if valid_space:
                if abs(dt) <= w300:
                    j = '300'
                elif abs(dt) <= w100:
                    j = '100'
                elif abs(dt) <= w50:
                    j = '50'
                else:
                    j = 'miss'
            else:
                j = 'miss'

            judgements.append(j)
            if j != 'miss':
                hit_errors.append(dt)
                hit_times.append(t_obj)
        else:
            judgements.append('miss')

    c300 = judgements.count('300')
    c100 = judgements.count('100')
    c50 = judgements.count('50')
    cmiss = judgements.count('miss')

    errs = np.array(hit_errors) if hit_errors else np.array([0.0])
    mean_err = float(np.mean(errs))
    std_err = float(np.std(errs))
    abs_errs = np.abs(errs)
    p50 = float(np.percentile(abs_errs, 50))
    p95 = float(np.percentile(abs_errs, 95))
    p99 = float(np.percentile(abs_errs, 99))

    # Drift slope (linear regression: dt vs t_obj)
    if len(hit_times) > 1:
        slope, intercept = np.polyfit(np.array(hit_times) / 1000.0, errs, 1) # ms error per second
    else:
        slope = 0.0

    return {
        'osr_header': {
            'count_300': replay['count_300'],
            'count_100': replay['count_100'],
            'count_50': replay['count_50'],
            'count_miss': replay['count_miss'],
            'score': replay['score'],
            'max_combo': replay['max_combo']
        },
        'simulator_judgements': {
            'count_300': c300,
            'count_100': c100,
            'count_50': c50,
            'count_miss': cmiss,
            'total_objects': len(objects)
        },
        'timing_metrics': {
            'sample_count': len(errs),
            'mean_ms': mean_err,
            'std_ms': std_err,
            'abs_p50_ms': p50,
            'abs_p95_ms': p95,
            'abs_p99_ms': p99,
            'drift_slope_ms_per_sec': float(slope),
            'drift_slope_ms_per_min': float(slope * 60.0)
        }
    }

if __name__ == '__main__':
    if len(sys.argv) < 3:
        print("Usage: python analyze_osr.py <replay.osr> <beatmap.osu>")
        sys.exit(1)
    res = analyze_replay_against_beatmap(sys.argv[1], sys.argv[2])
    print(json.dumps(res, indent=2))
