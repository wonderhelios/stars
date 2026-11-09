from pathlib import Path
BASE = Path(__file__).resolve().parent
"""训练 ML 模型（LightGBM）→ 导出 model.json（给 predict.py 加载）。"""
import numpy as np,pandas as pd,datetime,warnings,json
import lightgbm as lgb
warnings.filterwarnings('ignore')
d=np.load(str(BASE/'features.npz'))
ts=d['ts']; coins=d['coins']; labels=d['labels']; members=d['members']
n,k=labels.shape
feat_names=[f for f in d.files if f.startswith('F_')]
X=np.column_stack([d[f].reshape(-1) for f in feat_names])
y=labels.reshape(-1); day=np.repeat(np.arange(n),k)
ts_dt=np.array([np.datetime64(datetime.datetime.utcfromtimestamp(t/1000)) for t in ts])
train_end=np.datetime64('2026-04-30')
train_start=np.datetime64('2023-06-01')
elig=(ts_dt[day]>=train_start)&(ts_dt[day]<=train_end)&np.isfinite(y)
dates=np.unique(ts_dt[day][elig]); split=dates[len(dates)*4//5]
train_mask=elig&(ts_dt[np.minimum(day+3,n-1)]<split)
valid_mask=(ts_dt[day]>=split)&(ts_dt[day]<=train_end)&np.isfinite(y)
params={'objective':'regression','metric':'rmse','learning_rate':0.05,'num_leaves':31,
        'min_data_in_leaf':200,'feature_fraction':0.8,'bagging_fraction':0.8,'bagging_freq':1,
        'lambda_l2':10,'seed':42,'num_threads':-1,'verbose':-1}
tr=lgb.Dataset(X[train_mask],label=y[train_mask],feature_name=feat_names)
va=lgb.Dataset(X[valid_mask],label=y[valid_mask],reference=tr)
model=lgb.train(params,tr,num_boost_round=1000,valid_sets=[va],callbacks=[lgb.early_stopping(100,verbose=False)])
print(f"最佳轮数 {model.best_iteration}, 树数 {model.num_trees()}")
# dump 成简洁格式: 每棵树 = 节点列表(特征idx/阈值/左右子/叶子值)
dump=model.dump_model()
trees=[]
for tinfo in dump['tree_info']:
    nodes=[]  # 用 BFS/DFS 展开成数组
    def walk(node):
        if 'leaf_value' in node:
            idx=len(nodes); nodes.append({'leaf':float(node['leaf_value'])})
            return idx
        idx=len(nodes); nodes.append({'feat':int(node['split_feature']),'thr':float(node['threshold']),
            'left':-1,'right':-1,'miss':1 if node.get('default_left',True) else 0})
        li=walk(node['left_child']); ri=walk(node['right_child'])
        nodes[idx]['left']=li; nodes[idx]['right']=ri
        return idx
    walk(tinfo['tree_structure'])
    trees.append(nodes)
out={'feature_names':[f.replace('F_','') for f in feat_names],
     'trees':trees, 'shrinkage':0.05}
json.dump(out, open(str(BASE/'model.json'), 'w'))
print(f"导出 {len(trees)} 棵树, 总节点 {sum(len(t) for t in trees)}")
print(f"特征名: {out['feature_names']}")
# 验证: 用导出的 JSON 自己推理, 对比 model.predict
def predict_json(x, out):
    s=0.0
    for nodes in out['trees']:
        idx=0
        while 'leaf' not in nodes[idx]:
            n=nodes[idx]
            fv=x[n['feat']]
            if np.isnan(fv):
                idx=n['right'] if n['miss']==0 else n['left']  # miss=1→left, miss=0→right
            else:
                idx=n['left'] if fv<=n['thr'] else n['right']
        s+=nodes[idx]['leaf']
    return s
# 取最后一天有标签的行验证
sel=np.where(valid_mask)[0][-100:]
pred_model=model.predict(X[sel])
pred_json=[predict_json(X[i], out) for i in sel]
diff=np.max(np.abs(np.array(pred_model)-np.array(pred_json)))
print(f"JSON 推理 vs model.predict 最大误差: {diff:.6f}")
print("导出完成 model.json")
